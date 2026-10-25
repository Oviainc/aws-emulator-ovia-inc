// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-keyspacesstreams-keyspacesstreams service.
//!
//! KeyspacesStreams binds no operation that creates a stream: Amazon Keyspaces creates
//! one when a table is created or altered with a CDC specification, and this service
//! only reports it and serves its records. The write side therefore lives HERE —
//! [`ComAmazonawsKeyspacesstreamsKeyspacesstreamsState::enable_stream`],
//! [`ComAmazonawsKeyspacesstreamsKeyspacesstreamsState::disable_stream`],
//! [`ComAmazonawsKeyspacesstreamsKeyspacesstreamsState::append_change_record`] and
//! [`ComAmazonawsKeyspacesstreamsKeyspacesstreamsState::split_shard`] — and it is ONE
//! store in the shared engine, so the stream the Keyspaces control plane
//! (`service-com-amazonaws-keyspaces-keyspacesservice`, whose `CreateTable` and
//! `UpdateTable` drive it from their `cdcSpecification`) enables is exactly the stream
//! `src/ops/` reports. No handler in THIS crate writes a stream, a shard or a record:
//! the control plane and this crate's tests are what populate them.

use std::collections::hash_map::RandomState;
use std::collections::{BTreeMap, HashMap};
use std::hash::BuildHasher;
use std::time::{Duration, SystemTime};

use crate::ops::types::{KeyspacesCellValue, KeyspacesRow};

/// How long a shard iterator this service mints stays usable.
///
/// "Each shard iterator is valid for 15 minutes after creation." — the
/// `shardIterator` docs on `GetShardIterator`'s output and `GetRecords`'s input in
/// this service's Smithy model.
pub const SHARD_ITERATOR_LIFETIME: Duration = Duration::from_secs(15 * 60);

/// How long a stream stays in the transient status it is created in before the clock
/// moves it to its terminal one.
///
/// CHOSEN, not documented by AWS: it sits inside the 250..=750 ms band this project
/// fixes for a clock-owed transition, so a caller that reads the stream immediately
/// after it is enabled still observes `ENABLING`.
pub const STATUS_TRANSITION_DELAY: Duration = Duration::from_millis(400);

/// `StreamStatus` = `ENABLING`: the status a stream is created in, while Amazon
/// Keyspaces is bringing change data capture up for its table.
///
/// The four constants below are the whole of the model's `StreamStatus` value set
/// (`crate::ops::types::STREAM_STATUS_VALUES`), which is also the whole of the
/// Keyspaces control plane's `CdcStatus` value set: a table's CDC status IS the status
/// of the stream this store holds for it, which is why neither side keeps a second copy.
pub const STREAM_STATUS_ENABLING: &str = "ENABLING";
/// `StreamStatus` = `ENABLED`: the status an enabling stream settles into, in which it
/// captures the changes made to its table.
pub const STREAM_STATUS_ENABLED: &str = "ENABLED";
/// `StreamStatus` = `DISABLING`: the status a stream enters when CDC is turned off for
/// its table. It captures nothing more from that instant on.
pub const STREAM_STATUS_DISABLING: &str = "DISABLING";
/// `StreamStatus` = `DISABLED`: the status a disabling stream settles into. The records
/// it already captured stay readable — a consumer can drain them — but no shard of it
/// accepts another.
pub const STREAM_STATUS_DISABLED: &str = "DISABLED";

/// The `eventVersion` stamped on every record this emulator captures: the version of
/// the record FORMAT, so it is a property of the emulator and not of one record.
pub const RECORD_FORMAT_VERSION: &str = "1.0";

/// How long a pagination token this service issued stays resumable.
///
/// CHOSEN, not documented by AWS: it matches the shard iterator's documented lifetime,
/// and the model's own `ExpiredNextToken` is the refusal a caller gets after it.
pub const PAGE_TOKEN_LIFETIME: Duration = Duration::from_secs(15 * 60);

/// The most iterator registrations this service keeps. A caller mints one per
/// `GetRecords` poll, so the registry is pruned on every mint — see
/// [`prune_registry`], which evicts unconditionally and so bounds the memory a caller
/// can make this service hold however fast it polls.
pub const MAX_TRACKED_ITERATORS: usize = 4096;

/// The most pagination cursors this service keeps, bounded for the same reason and in
/// the same way: a caller mints one per page it asks for.
pub const MAX_TRACKED_PAGE_TOKENS: usize = 4096;

/// The most streams this service keeps for ONE table: its live stream, plus the
/// disabled ones it had before.
///
/// An EMULATOR ceiling, not an AWS quota: turning CDC off and on again mints a new
/// stream and keeps the disabled one (its records stay readable), so a caller looping on
/// `UpdateTable` would otherwise grow this store without end. Past the cap the table's
/// OLDEST disabled stream is dropped — never a live one, and never a newer one — so the
/// stream a running consumer is draining is still there.
pub const MAX_STREAMS_PER_TABLE: usize = 8;

/// A status change the clock owes a stream, with no further call: AWS reports a
/// freshly enabled stream as `ENABLING` and moves it to `ENABLED` itself.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    /// The instant from which the new status is owed.
    pub due: SystemTime,
    /// The `StreamStatus` the stream moves to.
    pub status: String,
}

/// One change to a captured row, as the table's write path reports it to the stream.
///
/// The stream itself assigns the sequence number, the creation time and the event
/// version, and projects the images its `streamViewType` publishes — so a caller
/// hands over only what the change itself says.
#[derive(Debug, Clone)]
pub struct ChangeRecord {
    /// An `OriginType` value: what made the change (`USER`, `TTL`, `REPLICATION`).
    pub origin: String,
    /// The partition key columns of the affected row.
    pub partition_keys: BTreeMap<String, KeyspacesCellValue>,
    /// The clustering key columns of the affected row.
    pub clustering_keys: BTreeMap<String, KeyspacesCellValue>,
    /// The row after the change, when the change left one.
    pub new_image: Option<KeyspacesRow>,
    /// The row before the change, when there was one.
    pub old_image: Option<KeyspacesRow>,
}

impl Default for ChangeRecord {
    fn default() -> Self {
        Self {
            // `USER` is the origin of a change made by a client statement; the other
            // `OriginType` values are for TTL expiry and multi-region replication.
            origin: "USER".to_string(),
            partition_keys: BTreeMap::new(),
            clustering_keys: BTreeMap::new(),
            new_image: None,
            old_image: None,
        }
    }
}

/// A captured change record, as this service stores and reports it.
#[derive(Debug, Clone)]
pub struct RecordEntry {
    /// Unique within the shard and increasing with capture order.
    pub sequence_number: String,
    pub event_version: String,
    pub created_at: SystemTime,
    pub origin: String,
    pub partition_keys: BTreeMap<String, KeyspacesCellValue>,
    pub clustering_keys: BTreeMap<String, KeyspacesCellValue>,
    pub new_image: Option<KeyspacesRow>,
    pub old_image: Option<KeyspacesRow>,
}

/// One shard of a stream: a contiguous run of records, open (still accepting them) or
/// closed (its range has an end).
#[derive(Debug, Clone)]
pub struct ShardEntry {
    pub shard_id: String,
    /// The shards this one evolved from, when it was created by resharding.
    pub parent_shard_ids: Vec<String>,
    /// The shard's position in the stream, minted when the shard opens. It precedes
    /// every record the shard holds.
    pub starting_sequence_number: String,
    /// `None` while the shard is open — an open-ended range.
    pub ending_sequence_number: Option<String>,
    /// Kept in capture order, which is sequence-number order.
    pub records: Vec<RecordEntry>,
}

impl ShardEntry {
    /// Whether the shard still accepts records (its range has no end).
    pub fn is_open(&self) -> bool {
        self.ending_sequence_number.is_none()
    }

    /// Where `sequence_number` sits in this shard.
    pub fn locate(&self, sequence_number: &str) -> Result<SequenceLocation, SequenceLookupError> {
        if sequence_number == self.starting_sequence_number {
            return Ok(SequenceLocation::ShardStart);
        }
        if let Some(index) = self
            .records
            .iter()
            .position(|record| record.sequence_number == sequence_number)
        {
            return Ok(SequenceLocation::Record(index));
        }
        match compare_sequence_numbers(sequence_number, &self.starting_sequence_number) {
            Some(std::cmp::Ordering::Less) => Err(SequenceLookupError::Trimmed),
            _ => Err(SequenceLookupError::NotInShard),
        }
    }
}

/// Where a requested sequence number sits in a shard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceLocation {
    /// The shard's own starting sequence number: before every record it holds.
    ShardStart,
    /// The record at this index.
    Record(usize),
}

/// Why a sequence number names no position in a shard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceLookupError {
    /// Older than the shard's starting sequence number: the data is gone.
    Trimmed,
    /// Within the shard's range but not a sequence number the shard issued.
    NotInShard,
}

/// A change data capture stream for one Keyspaces table.
#[derive(Debug, Clone)]
pub struct StreamEntry {
    pub stream_arn: String,
    /// The point in time the stream was created, as its unique label.
    pub stream_label: String,
    /// The region whose endpoint the table (and so this stream) lives in; part of the
    /// stream's identity, because the same keyspace/table name in another region is
    /// another table.
    pub region: String,
    pub keyspace_name: String,
    pub table_name: String,
    /// A `StreamStatus` value.
    pub stream_status: String,
    /// A `StreamViewType` value: which images the stream's records carry.
    pub stream_view_type: String,
    pub creation_request_date_time: SystemTime,
    /// The status the clock still owes this stream, if any.
    pub pending_status: Option<PendingStatus>,
    /// Oldest shard first; a shard's children follow it.
    pub shards: Vec<ShardEntry>,
}

impl StreamEntry {
    /// The shard `shard_id` names, if this stream holds it.
    pub fn shard(&self, shard_id: &str) -> Option<&ShardEntry> {
        self.shards.iter().find(|shard| shard.shard_id == shard_id)
    }

    /// Whether this stream is still following its table: a `StreamStatus` of
    /// [`STREAM_STATUS_ENABLING`] or [`STREAM_STATUS_ENABLED`].
    ///
    /// A [`STREAM_STATUS_DISABLING`] or [`STREAM_STATUS_DISABLED`] stream has stopped
    /// capturing changes for good — CDC was turned off for its table — so it is not the
    /// stream a later enable re-asserts, and it accepts no further record.
    pub fn is_live(&self) -> bool {
        matches!(
            self.stream_status.as_str(),
            STREAM_STATUS_ENABLING | STREAM_STATUS_ENABLED
        )
    }
}

/// A registered shard iterator: an immutable (stream, shard, position) bookmark.
///
/// `GetRecords` never moves one — it mints a new iterator at the advanced position, so
/// re-reading with the same iterator returns the same records, as AWS does.
#[derive(Debug, Clone)]
pub struct IteratorEntry {
    pub stream_arn: String,
    pub shard_id: String,
    /// Index into [`ShardEntry::records`] of the next record to return.
    pub next_index: usize,
    pub created_at: SystemTime,
    /// The ordinal this entry was minted with: it fixes the order the registry evicts
    /// entries in, so eviction does not depend on the clock's resolution.
    pub minted: u64,
}

impl IteratorEntry {
    /// Whether this iterator is past [`SHARD_ITERATOR_LIFETIME`] at `now`.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        now.duration_since(self.created_at)
            .is_ok_and(|age| age > SHARD_ITERATOR_LIFETIME)
    }
}

/// The `shardFilter` a page of shards was issued for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardFilterScope {
    /// A `ShardFilterType` value.
    pub filter_type: Option<String>,
    pub shard_id: Option<String>,
}

/// The query a pagination token continues. A token presented to a different query is
/// refused rather than silently applied to it — and the request's region is part of the
/// query, because the enumeration it pages over is region-scoped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageScope {
    /// A `ListStreams` enumeration, with the region and filters it was issued for.
    Streams {
        region: Option<String>,
        keyspace_name: Option<String>,
        table_name: Option<String>,
    },
    /// A `GetStream` shard enumeration.
    Shards {
        region: Option<String>,
        stream_arn: String,
        shard_filter: Option<ShardFilterScope>,
    },
}

/// What an issued pagination token resumes: the query, and the item the previous page
/// ended at.
///
/// The model types these tokens `StreamArnToken` and `ShardIdToken` — a token anchored
/// to an item's IDENTITY, not to a count — so a page resumes "after this stream ARN /
/// after this shard id". That cannot silently skip or repeat an item when the list
/// gains one between two pages, which a bare offset does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    pub scope: PageScope,
    /// The id of the last item the previous page returned: a stream ARN for a
    /// `ListStreams` page, a shard id for a `GetStream` page.
    pub after: String,
}

/// Where a page resumes: just after the item the previous page ended at.
///
/// `None` when the enumeration no longer holds that item, which the caller can only
/// refuse: resuming by position instead would skip or repeat items, and no ordering of
/// the ids is assumed here beyond the enumeration's own.
pub fn resume_index(ids_in_order: &[&str], after: &str) -> Option<usize> {
    ids_in_order
        .iter()
        .position(|id| *id == after)
        .map(|index| index + 1)
}

/// A pagination token's registration: what it resumes, when it was issued (so it can
/// expire) and the ordinal it was minted with (so eviction is first-in-first-out).
#[derive(Debug, Clone)]
struct PageTokenEntry {
    cursor: PageCursor,
    created_at: SystemTime,
    minted: u64,
}

/// Why a presented `nextToken` cannot be resumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTokenError {
    /// This service holds no token with that value: it was never issued, or the
    /// registry has since evicted it. Indistinguishable from a fabricated token, so it
    /// is reported as a malformed one.
    NotIssued,
    /// Issued, and still held, but past [`PAGE_TOKEN_LIFETIME`].
    Expired,
    /// Issued, but for a different query than the one presenting it.
    WrongQuery,
}

/// Order two sequence numbers the way a shard orders its records: numerically, so the
/// comparison does not depend on how many leading zeros a caller sent. `None` when
/// either side is not a decimal sequence number.
fn compare_sequence_numbers(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    let decimal = |value: &str| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit());
    if !decimal(left) || !decimal(right) {
        return None;
    }
    let left_digits = left.trim_start_matches('0');
    let right_digits = right.trim_start_matches('0');
    Some(
        left_digits
            .len()
            .cmp(&right_digits.len())
            .then_with(|| left_digits.cmp(right_digits)),
    )
}

/// Bound a mint-on-request registry to `capacity`, evicting first-minted first.
///
/// Unconditional: eviction cannot be blocked by every entry still being live, so a
/// caller that mints faster than entries expire still cannot grow the registry past
/// its cap. Because an expired entry was always minted before a live one,
/// first-minted-first evicts the expired entries before any live one.
fn prune_registry<V>(
    registry: &mut HashMap<String, V>,
    capacity: usize,
    minted: impl Fn(&V) -> u64,
) {
    while registry.len() > capacity {
        let first_minted = registry
            .iter()
            .min_by_key(|(_, entry)| minted(entry))
            .map(|(token, _)| token.clone());
        match first_minted {
            Some(token) => registry.remove(&token),
            None => return,
        };
    }
}

/// Milliseconds since the Unix epoch, for the timestamp inside a minted identifier.
fn epoch_millis(at: SystemTime) -> u128 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// Per-service state for com-amazonaws-keyspacesstreams-keyspacesstreams. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKeyspacesstreamsKeyspacesstreamsState {
    /// Every stream, keyed by ARN so enumeration has one stable order.
    streams: BTreeMap<String, StreamEntry>,
    /// Shard iterators this service has issued, keyed by their opaque token.
    iterators: HashMap<String, IteratorEntry>,
    /// Pagination tokens this service has issued, keyed by their opaque value.
    page_tokens: HashMap<String, PageTokenEntry>,
    /// Mints sequence numbers, shard ids and tokens; every one is distinct.
    next_id: u64,
    /// Per-process hash keys, so a minted token cannot be guessed from the counter.
    token_keys: RandomState,
}

impl ComAmazonawsKeyspacesstreamsKeyspacesstreamsState {
    // ---------------------------------------------------------------- write side

    /// Enable change data capture for a table: mint the stream Amazon Keyspaces
    /// creates when a table is created or altered with a CDC specification, with one
    /// open shard and no records yet, and return its ARN.
    ///
    /// Idempotent per region/keyspace/table while the table's stream is LIVE
    /// ([`StreamEntry::is_live`]), the way re-asserting a table's CDC specification is:
    /// the table keeps that stream and its label. A table whose stream was DISABLED
    /// gets a NEW one — `GetTable` reports the `latestStreamArn` of a table, so a
    /// re-enabled table has a later stream, and the disabled one keeps the records it
    /// captured. `stream_view_type` decides which images the stream's records carry, and
    /// must be one of the model's `StreamViewType` values — `None` for any other value,
    /// the way Keyspaces refuses a CDC specification it cannot honour, rather than
    /// storing a view type that would silently publish no image.
    pub fn enable_stream(
        &mut self,
        region: &str,
        keyspace_name: &str,
        table_name: &str,
        stream_view_type: &str,
    ) -> Option<String> {
        // Idempotence first: a table whose stream is still live keeps it whatever the
        // re-asserted view type says ("After you create the stream, you can't make
        // changes to this selection"), so a repeat call cannot be refused by a value
        // that only matters when a stream is created.
        if let Some(existing) = self.live_stream_for_table(region, keyspace_name, table_name) {
            return Some(existing.stream_arn.clone());
        }
        if !crate::ops::types::STREAM_VIEW_TYPE_VALUES.contains(&stream_view_type) {
            return None;
        }
        // A stream's label is the instant it was created, and it has to IDENTIFY the
        // stream inside its ARN, which is this map's key. Two streams of one table minted
        // inside the label's resolution would therefore collide — a table whose CDC is
        // turned off and straight back on is exactly that case — so the later one is
        // labelled at the next free instant: the label stays the creation time (to the
        // millisecond), every stream of a table has its own ARN, and no stream can
        // overwrite another's records or be answered in place of it.
        let mut created = SystemTime::now();
        let (stream_label, stream_arn) = loop {
            let stream_label = stream_label_for(created);
            // The resource path Amazon Keyspaces gives a table's stream:
            // `/keyspace/<keyspace>/table/<table>/stream/<label>`.
            let stream_arn = ::engine::emulator_arn(
                "cassandra",
                region,
                &format!("/keyspace/{keyspace_name}/table/{table_name}/stream/{stream_label}"),
            );
            if !self.streams.contains_key(&stream_arn) {
                break (stream_label, stream_arn);
            }
            created += Duration::from_millis(1);
        };
        let shard = self.new_shard(created, Vec::new());
        self.streams.insert(
            stream_arn.clone(),
            StreamEntry {
                stream_arn: stream_arn.clone(),
                stream_label,
                region: region.to_string(),
                keyspace_name: keyspace_name.to_string(),
                table_name: table_name.to_string(),
                // AWS returns a stream it has just created in a non-terminal status
                // and moves it on itself; `materialise_due_transitions` is what
                // completes it.
                stream_status: STREAM_STATUS_ENABLING.to_string(),
                stream_view_type: stream_view_type.to_string(),
                creation_request_date_time: created,
                pending_status: Some(PendingStatus {
                    due: created + STATUS_TRANSITION_DELAY,
                    status: STREAM_STATUS_ENABLED.to_string(),
                }),
                shards: vec![shard],
            },
        );
        // This table now holds one more stream than before, and one call adds at most
        // one, so the cap is applied here with a single eviction: the table's OLDEST
        // stream goes, never the live one just minted and never a newer one. Oldest is
        // decided by the creation instant the entry carries (ties broken by ARN, as
        // [`Self::stream_for_table`] breaks them), so the choice does not depend on how a
        // label happens to render.
        let held: Vec<(SystemTime, String)> = self
            .streams
            .values()
            .filter(|stream| {
                stream.region == region
                    && stream.keyspace_name == keyspace_name
                    && stream.table_name == table_name
            })
            .map(|stream| (stream.creation_request_date_time, stream.stream_arn.clone()))
            .collect();
        if held.len() > MAX_STREAMS_PER_TABLE {
            let oldest = held
                .iter()
                .min_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)))
                .map(|(_, stream_arn)| stream_arn.clone());
            if let Some(oldest) = oldest {
                self.forget_stream(&oldest);
            }
        }
        Some(stream_arn)
    }

    /// Disable change data capture for a table: the stream Amazon Keyspaces created for
    /// it stops capturing changes, and the ARN of that stream is returned.
    ///
    /// The stream enters [`STREAM_STATUS_DISABLING`] and the clock owes it
    /// [`STREAM_STATUS_DISABLED`] one [`STATUS_TRANSITION_DELAY`] later, exactly as
    /// enabling is a two-step the clock finishes. It is NOT removed: `StreamStatus`
    /// models `DISABLED`, which only a stream a caller can still read could ever
    /// report, so a consumer keeps draining the records captured before the disable —
    /// and every shard's range ends here, so nothing more can be appended.
    ///
    /// `None` when the table has no live stream: disabling CDC on a table that has none
    /// changes nothing.
    pub fn disable_stream(
        &mut self,
        region: &str,
        keyspace_name: &str,
        table_name: &str,
    ) -> Option<String> {
        let stream_arn = self
            .live_stream_for_table(region, keyspace_name, table_name)?
            .stream_arn
            .clone();
        let now = SystemTime::now();
        let stream = self.streams.get_mut(&stream_arn)?;
        stream.stream_status = STREAM_STATUS_DISABLING.to_string();
        stream.pending_status = Some(PendingStatus {
            due: now + STATUS_TRANSITION_DELAY,
            status: STREAM_STATUS_DISABLED.to_string(),
        });
        // A stream that captures nothing more has no open-ended range left: a shard's
        // end is its last record, or its own start when it never held one.
        for shard in stream.shards.iter_mut().filter(|shard| shard.is_open()) {
            shard.ending_sequence_number = Some(
                shard
                    .records
                    .last()
                    .map_or(shard.starting_sequence_number.clone(), |record| {
                        record.sequence_number.clone()
                    }),
            );
        }
        Some(stream_arn)
    }

    /// Forget every stream of one table, and answer with the ARNs forgotten.
    ///
    /// A stream follows one table's changes, so it cannot outlive that table: the
    /// Keyspaces control plane calls this when the table is deleted (and, per keyspace,
    /// when the keyspace that holds it is).
    pub fn forget_streams_for_table(
        &mut self,
        region: &str,
        keyspace_name: &str,
        table_name: &str,
    ) -> Vec<String> {
        self.forget_streams(|stream| {
            stream.region == region
                && stream.keyspace_name == keyspace_name
                && stream.table_name == table_name
        })
    }

    /// Forget every stream of every table of one keyspace, and answer with the ARNs
    /// forgotten: "The `DeleteKeyspace` operation deletes a keyspace and all of its
    /// tables", and a table's stream goes with the table.
    pub fn forget_streams_in_keyspace(
        &mut self,
        region: &str,
        keyspace_name: &str,
    ) -> Vec<String> {
        self.forget_streams(|stream| {
            stream.region == region && stream.keyspace_name == keyspace_name
        })
    }

    /// The ARNs of every stream this store holds for one table, in no particular order:
    /// this is a membership set, not a listing.
    ///
    /// A table's streams are capped here ([`MAX_STREAMS_PER_TABLE`]), and the Keyspaces
    /// control plane holds each stream's TAGS, so it prunes those against this list:
    /// that is what keeps a tag set from outliving the stream it belongs to.
    pub fn stream_arns_for_table(
        &self,
        region: &str,
        keyspace_name: &str,
        table_name: &str,
    ) -> Vec<String> {
        self.streams
            .values()
            .filter(|stream| {
                stream.region == region
                    && stream.keyspace_name == keyspace_name
                    && stream.table_name == table_name
            })
            .map(|stream| stream.stream_arn.clone())
            .collect()
    }

    /// Remove every stream `matches` selects, with the iterators bookmarking them, and
    /// answer with the ARNs removed.
    fn forget_streams(&mut self, matches: impl Fn(&StreamEntry) -> bool) -> Vec<String> {
        let forgotten: Vec<String> = self
            .streams
            .values()
            .filter(|stream| matches(stream))
            .map(|stream| stream.stream_arn.clone())
            .collect();
        for stream_arn in &forgotten {
            self.forget_stream(stream_arn);
        }
        forgotten
    }

    /// Remove one stream and the iterators bookmarking it.
    ///
    /// An iterator is a bookmark INTO a stream, so one into a stream that is gone can
    /// never be served again; dropping it here keeps the registry from holding them.
    fn forget_stream(&mut self, stream_arn: &str) {
        self.streams.remove(stream_arn);
        self.iterators
            .retain(|_, entry| entry.stream_arn != stream_arn);
    }

    /// Capture one change to a row of the table `stream_arn`'s stream follows, as a
    /// write to that table does, and return the sequence number the stream assigned.
    ///
    /// The images are projected by the stream's `streamViewType`, so a `KEYS_ONLY`
    /// stream stores no image and a `NEW_IMAGE` stream stores only the new one.
    /// `None` when no stream has that ARN, or when it has no open shard.
    pub fn append_change_record(
        &mut self,
        stream_arn: &str,
        change: ChangeRecord,
    ) -> Option<String> {
        // Resolve the target BEFORE minting, so a call naming no stream (or a stream
        // with no open shard) leaves no gap in the minted sequence space.
        if !self.has_open_shard(stream_arn) {
            return None;
        }
        let created = SystemTime::now();
        let sequence_number = self.mint_sequence_number();
        let stream = self.streams.get_mut(stream_arn)?;
        let (new_image, old_image) = project_images(&stream.stream_view_type, &change);
        let shard = stream.shards.iter_mut().rev().find(|s| s.is_open())?;
        shard.records.push(RecordEntry {
            sequence_number: sequence_number.clone(),
            event_version: RECORD_FORMAT_VERSION.to_string(),
            created_at: created,
            origin: change.origin,
            partition_keys: change.partition_keys,
            clustering_keys: change.clustering_keys,
            new_image,
            old_image,
        });
        Some(sequence_number)
    }

    /// Reshard a stream: close its newest open shard and open a child of it, the way
    /// Keyspaces rolls a shard over. Returns the new shard's id, or `None` when no
    /// stream has that ARN or it has no open shard.
    pub fn split_shard(&mut self, stream_arn: &str) -> Option<String> {
        // Resolve the shard that is rolling over BEFORE minting its child, so a call
        // naming no stream (or no open shard) leaves no gap in the minted id space and
        // the child can be built with its parent already named.
        let parent_id = self
            .streams
            .get(stream_arn)?
            .shards
            .iter()
            .rev()
            .find(|shard| shard.is_open())?
            .shard_id
            .clone();
        let child = self.new_shard(SystemTime::now(), vec![parent_id]);
        let child_id = child.shard_id.clone();
        let stream = self.streams.get_mut(stream_arn)?;
        let parent = stream.shards.iter_mut().rev().find(|s| s.is_open())?;
        parent.ending_sequence_number = Some(
            parent
                .records
                .last()
                .map_or(parent.starting_sequence_number.clone(), |record| {
                    record.sequence_number.clone()
                }),
        );
        stream.shards.push(child);
        Some(child_id)
    }

    /// Apply every status change the clock already owes a stream at `now`.
    ///
    /// A reporting operation calls this before it reads: the change was owed at this
    /// instant whether or not anyone called.
    pub fn materialise_due_transitions(&mut self, now: SystemTime) {
        for stream in self.streams.values_mut() {
            if stream
                .pending_status
                .as_ref()
                .is_some_and(|pending| now >= pending.due)
            {
                if let Some(pending) = stream.pending_status.take() {
                    stream.stream_status = pending.status;
                }
            }
        }
    }

    /// TEST SEAM — no handler calls this. Shift every issued shard iterator and
    /// pagination cursor's creation instant back by `age`, so a test can reach the
    /// lifetimes this service documents without waiting for them. It changes nothing a
    /// response reports except how old an already-issued token is.
    pub fn backdate_issued_tokens(&mut self, age: Duration) {
        for entry in self.iterators.values_mut() {
            entry.created_at = entry.created_at.checked_sub(age).unwrap_or(entry.created_at);
        }
        for entry in self.page_tokens.values_mut() {
            entry.created_at = entry.created_at.checked_sub(age).unwrap_or(entry.created_at);
        }
    }

    // ----------------------------------------------------------------- read side

    /// Every stream the request's region can see, in ARN order.
    ///
    /// A Keyspaces table — and so its stream — belongs to one region, and a request
    /// reaches one region's endpoint, so a stream of another region is not visible.
    /// `region` is `None` when the request names no region (it is unsigned), and then
    /// nothing is hidden: the emulator has no second signal to scope it by.
    pub fn streams_in<'a>(
        &'a self,
        region: Option<&'a str>,
    ) -> impl Iterator<Item = &'a StreamEntry> {
        self.streams
            .values()
            .filter(move |stream| region.is_none_or(|region| region == stream.region))
    }

    /// Every stream, in ARN order.
    pub fn streams(&self) -> impl Iterator<Item = &StreamEntry> {
        self.streams.values()
    }

    /// The stream `stream_arn` names, if the request's region can see it (see
    /// [`Self::streams_in`]).
    pub fn stream_in(&self, region: Option<&str>, stream_arn: &str) -> Option<&StreamEntry> {
        self.streams
            .get(stream_arn)
            .filter(|stream| region.is_none_or(|region| region == stream.region))
    }

    /// The stream `stream_arn` names.
    pub fn stream(&self, stream_arn: &str) -> Option<&StreamEntry> {
        self.streams.get(stream_arn)
    }

    /// The LATEST stream following one table in one region, if that table ever had CDC
    /// enabled.
    ///
    /// A table can have had more than one stream over its life — CDC turned off and on
    /// again mints a new one — and the one that speaks for the table now is the most
    /// recently created, which is what Keyspaces reports as a table's `latestStreamArn`.
    /// Ties (two streams created inside the clock's resolution) are broken by ARN, so
    /// the answer is total and stable rather than dependent on map order.
    pub fn stream_for_table(
        &self,
        region: &str,
        keyspace_name: &str,
        table_name: &str,
    ) -> Option<&StreamEntry> {
        self.streams
            .values()
            .filter(|stream| {
                stream.region == region
                    && stream.keyspace_name == keyspace_name
                    && stream.table_name == table_name
            })
            .max_by(|left, right| {
                left.creation_request_date_time
                    .cmp(&right.creation_request_date_time)
                    .then_with(|| left.stream_arn.cmp(&right.stream_arn))
            })
    }

    /// The stream that is still capturing one table's changes, if CDC is on for it: the
    /// table's latest stream, when that stream [is live][StreamEntry::is_live].
    ///
    /// A disabled stream is not re-enabled by a later CDC specification — that mints a
    /// new stream — so the enable/disable write side asks for this one, not for
    /// [`Self::stream_for_table`].
    pub fn live_stream_for_table(
        &self,
        region: &str,
        keyspace_name: &str,
        table_name: &str,
    ) -> Option<&StreamEntry> {
        self.stream_for_table(region, keyspace_name, table_name)
            .filter(|stream| stream.is_live())
    }

    /// Whether a stream with this ARN exists and still accepts records.
    fn has_open_shard(&self, stream_arn: &str) -> bool {
        self.streams
            .get(stream_arn)
            .is_some_and(|stream| stream.shards.iter().any(ShardEntry::is_open))
    }

    /// The iterator a `shardIterator` token names, expired or not.
    pub fn shard_iterator(&self, token: &str) -> Option<&IteratorEntry> {
        self.iterators.get(token)
    }

    // -------------------------------------------------------------- token minting

    /// Register a bookmark at `next_index` in a shard and return its opaque token.
    ///
    /// The registry is bounded on every mint, so repeated polling cannot grow it.
    pub fn mint_shard_iterator(
        &mut self,
        stream_arn: &str,
        shard_id: &str,
        next_index: usize,
    ) -> String {
        let token = self.mint_token("ksstreams-iterator-");
        let minted = self.next_id;
        self.iterators.insert(
            token.clone(),
            IteratorEntry {
                stream_arn: stream_arn.to_string(),
                shard_id: shard_id.to_string(),
                next_index,
                created_at: SystemTime::now(),
                minted,
            },
        );
        prune_registry(&mut self.iterators, MAX_TRACKED_ITERATORS, |entry| {
            entry.minted
        });
        token
    }

    /// Register the continuation of a paginated query and return its opaque token.
    ///
    /// The token is 95 characters, inside the 80..3000 bound the model puts on
    /// `nextToken`, and carries no part of the cursor: the cursor is held here. This
    /// registry is bounded on every mint too, so a caller paging in a loop cannot grow
    /// it; a cursor that is evicted or past [`PAGE_TOKEN_LIFETIME`] is refused as an
    /// expired `nextToken` rather than resumed.
    pub fn mint_page_token(&mut self, cursor: PageCursor) -> String {
        let token = self.mint_token("ksstreams-page-");
        let minted = self.next_id;
        self.page_tokens.insert(
            token.clone(),
            PageTokenEntry {
                cursor,
                created_at: SystemTime::now(),
                minted,
            },
        );
        prune_registry(&mut self.page_tokens, MAX_TRACKED_PAGE_TOKENS, |entry| {
            entry.minted
        });
        token
    }

    /// The item a presented `nextToken` resumes `scope` after; `None` when there is no
    /// token, i.e. the query starts at its first item.
    pub fn resolve_page_token(
        &self,
        token: Option<&str>,
        scope: &PageScope,
    ) -> Result<Option<String>, PageTokenError> {
        let Some(token) = token else {
            return Ok(None);
        };
        let entry = self
            .page_tokens
            .get(token)
            .ok_or(PageTokenError::NotIssued)?;
        let expired = SystemTime::now()
            .duration_since(entry.created_at)
            .is_ok_and(|age| age > PAGE_TOKEN_LIFETIME);
        if expired {
            return Err(PageTokenError::Expired);
        }
        if &entry.cursor.scope != scope {
            return Err(PageTokenError::WrongQuery);
        }
        Ok(Some(entry.cursor.after.clone()))
    }

    // ------------------------------------------------------------------ internals

    /// An open shard with no records, positioned after every shard minted so far.
    fn new_shard(&mut self, created: SystemTime, parent_shard_ids: Vec<String>) -> ShardEntry {
        ShardEntry {
            shard_id: self.mint_shard_id(created),
            parent_shard_ids,
            starting_sequence_number: self.mint_sequence_number(),
            ending_sequence_number: None,
            records: Vec::new(),
        }
    }

    /// The next value of the one counter every minted identifier is distinct by.
    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// A 21-digit sequence number: the model's minimum length, increasing with the
    /// counter, so the order of two sequence numbers is the order they were minted in.
    fn mint_sequence_number(&mut self) -> String {
        format!("{:021}", self.next_id())
    }

    /// A shard id of the form `shardId-<epoch millis>-<8 hex>` (37 characters, inside
    /// the model's 28..65 bound), as Keyspaces shard ids are shaped.
    fn mint_shard_id(&mut self, created: SystemTime) -> String {
        let millis = epoch_millis(created);
        let suffix = self.mint_hash(0) as u32;
        format!("shardId-{millis:020}-{suffix:08x}")
    }

    /// 64 bits of this state's keyed hash over the counter and `salt`.
    fn mint_hash(&mut self, salt: u64) -> u64 {
        let id = self.next_id();
        self.token_keys.hash_one((id, salt))
    }

    /// `prefix` plus 80 hex characters of keyed hash: distinct per mint, opaque to the
    /// caller, and (at 95 characters) inside the 80..3000 bound the model puts on a
    /// pagination token.
    fn mint_token(&mut self, prefix: &str) -> String {
        use ::std::fmt::Write as _;

        let id = self.next_id();
        let mut token = String::with_capacity(prefix.len() + 80);
        token.push_str(prefix);
        for salt in 0..5_u64 {
            let chunk = self.token_keys.hash_one((id, salt, prefix));
            // Writes into the reserved buffer; `write!` to a `String` cannot fail.
            let _ = write!(token, "{chunk:016x}");
        }
        token
    }
}

/// The images a stream of this `streamViewType` publishes for a change.
///
/// "NEW_AND_OLD_IMAGES - both versions of the row, before and after the change.
/// NEW_IMAGE - the version of the row after the change. OLD_IMAGE - the version of the
/// row before the change. KEYS_ONLY - the partition and clustering keys of the row
/// that was changed." — `GetStream`'s `streamViewType` doc.
fn project_images(
    stream_view_type: &str,
    change: &ChangeRecord,
) -> (Option<KeyspacesRow>, Option<KeyspacesRow>) {
    match stream_view_type {
        "NEW_AND_OLD_IMAGES" => (change.new_image.clone(), change.old_image.clone()),
        "NEW_IMAGE" => (change.new_image.clone(), None),
        "OLD_IMAGE" => (None, change.old_image.clone()),
        // KEYS_ONLY, and any view type a later model version adds, publish no image.
        _ => (None, None),
    }
}

/// A stream's label: "A timestamp that serves as a unique identifier for this stream
/// … The stream label represents the point in time when the stream was created."
/// (`GetStream`'s `streamLabel` doc.)
///
/// Rendered to the MILLISECOND (`2026-10-23T10:07:58.123`), whatever sub-second digits
/// the timestamp formatter happens to emit: the label has to identify the stream, and a
/// label at second resolution could not tell two streams of one table apart. Uniqueness
/// is settled by the caller ([`ComAmazonawsKeyspacesstreamsKeyspacesstreamsState::enable_stream`]),
/// which labels a colliding stream at the next free instant.
fn stream_label_for(created: SystemTime) -> String {
    let subsecond_millis = epoch_millis(created) % 1_000;
    ::aws_smithy_types::DateTime::from(created)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .map(|rendered| {
            let whole_seconds = rendered.trim_end_matches('Z');
            let whole_seconds = whole_seconds
                .split_once('.')
                .map_or(whole_seconds, |(seconds, _)| seconds);
            format!("{whole_seconds}.{subsecond_millis:03}")
        })
        .unwrap_or_else(|_| epoch_millis(created).to_string())
}

/// The region a request is signed for, when it says so.
///
/// Read from the SigV4 credential scope, which every signed AWS SDK request carries;
/// `None` for an unsigned request, which names no region at all.
pub fn request_region(parts: &::http::request::Parts) -> Option<String> {
    ::emulator_server::sigv4_credential_scope(&parts.headers).map(|scope| scope.region)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKeyspacesstreamsKeyspacesstreamsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKeyspacesstreamsKeyspacesstreamsState>)
}
