// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kinesis-kinesis-20131202 service.
//!
//! This module owns the stream records the implemented operations read and write: the
//! shard map `CreateStream` lays down, the ARN a stream is addressed by, the tags assigned
//! to it, the record log a producer appends to and a consumer drains, and the single place
//! where the `CREATING` -> `ACTIVE`, `UPDATING` -> `ACTIVE` and `DELETING` -> gone
//! transitions the clock owes a stream are decided ([`Stream::status`],
//! [`Stream::is_held`]).

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    ChildShard, EnhancedMetrics, HashKeyRange, Record, SequenceNumberRange, Shard,
    StreamDescription, StreamDescriptionSummary, StreamModeDetails, StreamSummary, Tag,
    WarmThroughputObject,
};

/// The region a stream is created in when the request carries no SigV4 credential scope
/// to read one from (the emulator's default region, as `engine::emulator_arn` documents).
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, which is also the service segment the `StreamARN`
/// pattern (`^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$`) requires.
pub const SERVICE_ARN_NAMESPACE: &str = "kinesis";

/// A stream that is still being created, spelled as `StreamStatus` models it
/// (`smithy_query(query="shape",shape="StreamStatus")`).
pub const STREAM_CREATING: &str = "CREATING";

/// A stream that is ready for read and write operations.
pub const STREAM_ACTIVE: &str = "ACTIVE";

/// A stream a `DeleteStream` has been accepted for, which is gone once the deletion the
/// call armed completes: "After a DeleteStream request, the specified stream is in the
/// DELETING state until Kinesis Data Streams completes the deletion" (`DeleteStream`).
pub const STREAM_DELETING: &str = "DELETING";

/// A stream whose shard map is being re-tiled: "Upon receiving the request, Kinesis Data
/// Streams returns immediately and sets the status of the stream to UPDATING. After the
/// update is complete, Kinesis Data Streams sets the status of the stream back to ACTIVE"
/// (`UpdateShardCount`).
pub const STREAM_UPDATING: &str = "UPDATING";

/// The `StreamMode` whose streams carry the shard count the caller asked for.
pub const STREAM_MODE_PROVISIONED: &str = "PROVISIONED";

/// The `StreamMode` whose shard count the service manages itself.
pub const STREAM_MODE_ON_DEMAND: &str = "ON_DEMAND";

/// The capacity mode a stream gets when the request names no `StreamModeDetails`: the
/// `CreateStream` doc makes the shard count the caller's to choose ("For the data streams
/// with a provisioned mode, you must specify the number of shards"), which is the
/// provisioned mode.
pub const DEFAULT_STREAM_MODE: &str = STREAM_MODE_PROVISIONED;

/// How long after the `CreateStream` call the stream's `CREATING` -> `ACTIVE` transition
/// falls due. A CHOSEN span, not an AWS-documented duration: it only has to be non-zero,
/// so a caller that reads the stream straight after creating it still observes
/// `CREATING`. The instant it falls due is stored on the record and never slept on.
pub const ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// How long after the `DeleteStream` call the stream's removal falls due, taking it from
/// `DELETING` to gone. A CHOSEN span, not an AWS-documented duration: it only has to be
/// non-zero, so a caller that reads the stream straight after deleting it still observes
/// `DELETING`. The instant it falls due is stored on the record and never slept on.
pub const DELETION_DELAY: Duration = Duration::from_millis(500);

/// How long after a shard-count change the stream's `UPDATING` -> `ACTIVE` transition
/// falls due. A CHOSEN span, not an AWS-documented duration ("the scaling action could
/// take a few minutes to complete" names no value): it only has to be non-zero, so a
/// caller that reads the stream straight after scaling it still observes `UPDATING`. The
/// instant it falls due is stored on the record and never slept on.
pub const RESHARD_DELAY: Duration = Duration::from_millis(500);

/// How many shard-count changes one stream may be scaled by inside [`RESHARD_WINDOW`]: by
/// default you cannot "scale more than ten times per rolling 24-hour period per stream"
/// (`UpdateShardCount`).
pub const MAX_RESHARDS_PER_WINDOW: usize = 10;

/// The rolling window [`MAX_RESHARDS_PER_WINDOW`] scaling actions are counted over: the
/// "rolling 24-hour period per stream" `UpdateShardCount` documents.
pub const RESHARD_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// The retention window a new stream gets, in hours: "By default, a stream's data records
/// are accessible for 24 hours from the time they are added to the stream"
/// (`IncreaseStreamRetentionPeriod`), which is also the minimum
/// `StreamDescription$RetentionPeriodHours` publishes.
pub const DEFAULT_RETENTION_PERIOD_HOURS: i32 = 24;

/// The shards an on-demand stream starts with. The capacity an on-demand stream starts
/// with is 4 MiB/s of writes, and a shard supports "writes up to 1,000 records per
/// second, up to a maximum data write total of 1 MiB per second" (`CreateStream`), so a
/// new on-demand stream's shard map holds four shards.
pub const ON_DEMAND_INITIAL_SHARDS: u32 = 4;

/// The write throughput one shard supports, in MiB/s: "Each shard can support writes up
/// to 1,000 records per second, up to a maximum data write total of 1 MiB per second"
/// (`CreateStream`). A stream's warm throughput is this times its open shard count.
pub const SHARD_WRITE_MI_BPS: i32 = 1;

/// The server-side encryption a new stream carries: `EncryptionType` is `NONE | KMS` and
/// `StartStreamEncryption` is what moves a stream to `KMS`, so a stream nothing has
/// encrypted reports `NONE`.
pub const ENCRYPTION_TYPE_NONE: &str = "NONE";

/// The most shards one stream of this emulator holds. `ShardCount` is modelled `range
/// 1..` with no maximum, so a create past this one is refused rather than allocated — and
/// the request's count is checked against it BEFORE a shard map is sized from it.
pub const MAX_SHARDS_PER_STREAM: u32 = 500;

/// The most shards one region of this emulator holds across every stream. The
/// `CreateStream` doc answers `LimitExceededException` when a request would "create more
/// shards than are authorized for your account", and this is the count this emulator
/// authorizes, at the order of AWS's own default per-region shard quota.
pub const MAX_SHARDS_PER_REGION: u32 = 1_000;

/// The most streams one region of this emulator holds. `CreateStream` is the only thing
/// that adds one and the store lives for the life of the process, so the store needs a
/// maximum entry count; the emulator's OWN ceiling, not a modelled limit.
pub const MAX_STREAMS_PER_REGION: usize = 500;

/// The most streams this emulator holds across EVERY region, whatever region each was
/// created in.
///
/// The per-region quotas above are partitioned by the region the CALLER's credential scope
/// names, which the caller chooses, so they bound no total on their own: these two
/// ceilings, which no request can partition, are what keep the store from growing without
/// end. The emulator's OWN ceilings, not modelled limits.
pub const MAX_STREAMS_TOTAL: usize = 1_000;

/// The most shards this emulator holds across every stream of every region — the largest
/// thing a create allocates, so this is the ceiling that bounds the store's memory.
pub const MAX_SHARDS_TOTAL: u32 = 4_000;

/// The most tags one stream carries: "A set of up to 50 key-value pairs to use to create
/// the tags" (`CreateStreamInput$Tags`), which is narrower than the `TagMap` `@length` of
/// 1..200 the boundary validator enforces.
pub const MAX_STREAM_TAGS: usize = 50;

/// The most tags `ListTagsForStream` returns in one call, which is also the page a request
/// naming no `Limit` gets: `Limit` is modelled `range 1..50`
/// (`smithy_query(query="operation",operation="ListTagsForStream")`), and a stream carries
/// at most [`MAX_STREAM_TAGS`] tags, so a request naming no limit gets all of them.
pub const MAX_LIST_TAGS_FOR_STREAM_LIMIT: i32 = 50;

/// How many shards `DescribeStream` returns when the request names no `Limit`: "The
/// maximum number of shards to return in a single call. The default value is 100"
/// (`DescribeStreamInput$Limit`).
pub const DEFAULT_DESCRIBE_STREAM_SHARD_LIMIT: i32 = 100;

/// The most shards `DescribeStream` returns in one call: "If you specify a value greater
/// than 100, at most 100 results are returned" (`DescribeStreamInput$Limit`).
pub const MAX_DESCRIBE_STREAM_SHARD_LIMIT: i32 = 100;

/// How many shards `ListShards` returns when the request names no `MaxResults`: "The
/// maximum number of shards to return in a single call. The default value is 1000"
/// (`ListShardsInput$MaxResults`).
pub const DEFAULT_LIST_SHARDS_LIMIT: i32 = 1_000;

/// The most shards `ListShards` returns in one call: "If you specify a value greater than
/// 1000, at most 1000 results are returned" (`ListShardsInput$MaxResults`), which the
/// modelled `range 1..10000` deliberately leaves room above.
pub const MAX_LIST_SHARDS_LIMIT: i32 = 1_000;

/// How long a pagination token of this service stays usable. `ListShards` is the operation
/// that publishes the lifetime — "Tokens expire after 300 seconds. When you obtain a value
/// for NextToken in the response to a call to ListShards, you have 300 seconds to use that
/// value" (`ListShardsOutput$NextToken`) — and a cursor of this service is a cursor
/// whichever listing minted it, so every one of them has that lifetime.
pub const PAGE_TOKEN_TTL: Duration = Duration::from_secs(300);

/// See [`PAGE_TOKEN_TTL`]: how long a `ListShards` token stays usable.
pub const LIST_SHARDS_TOKEN_TTL: Duration = PAGE_TOKEN_TTL;

/// How many streams `ListStreams` returns when the request names no `Limit`: "The maximum
/// number of streams to list. The default value is 100" (`ListStreamsInput$Limit`).
pub const DEFAULT_LIST_STREAMS_LIMIT: i32 = 100;

/// The most streams `ListStreams` returns in one call: "If you specify a value greater than
/// 100, at most 100 results are returned" (`ListStreamsInput$Limit`), which the modelled
/// `range 1..10000` deliberately leaves room above.
pub const MAX_LIST_STREAMS_LIMIT: i32 = 100;

/// How many records `GetRecords` returns when the request names no `Limit`: "The maximum
/// number of records to return. Specify a value of up to 10,000... The default value is
/// 10,000" (`GetRecordsInput$Limit`).
pub const DEFAULT_GET_RECORDS_LIMIT: i32 = 10_000;

/// The most records `GetRecords` returns in one call: "The maximum number of records that
/// can be returned per call is 10,000" (`GetRecords`).
pub const MAX_GET_RECORDS_LIMIT: i32 = 10_000;

/// The most bytes one `GetRecords` call answers with: "The maximum size of data that
/// GetRecords can return is 10 MiB" (`GetRecords`), so a page stops at this size even when
/// the limit would admit more records.
pub const MAX_GET_RECORDS_BYTES: usize = 10 * 1024 * 1024;

/// The most records one shard of this emulator holds. A record stays in the store until
/// its retention window passes, so the shard needs a ceiling its oldest records are
/// dropped at; it is the emulator's OWN ceiling, set at the most one `GetRecords` call can
/// return ([`MAX_GET_RECORDS_LIMIT`]).
pub const MAX_RECORDS_PER_SHARD: usize = 10_000;

/// The most record bytes one shard of this emulator holds, the second half of the ceiling
/// [`MAX_RECORDS_PER_SHARD`] opens: `Data` is modelled up to 10 MiB per record, so a count
/// alone does not bound the store's memory. Set above one maximum-size record, because the
/// record a write just stored is never the one the ceiling drops. With
/// [`MAX_SHARDS_TOTAL`] bounding how many shards exist at all, the two together bound what
/// every stream of every region holds. The emulator's OWN ceiling, not a modelled limit.
pub const MAX_RECORD_BYTES_PER_SHARD: usize = 16 * 1024 * 1024;

/// How many seconds an hour of a retention window holds, so the window a stream reports in
/// hours can be measured against stored instants.
const SECONDS_PER_HOUR: u64 = 3_600;

/// How a stream is keyed: the region its creating request was signed for, and the name
/// that request carried.
///
/// "The stream name identifies the stream. The name is scoped to the Amazon Web Services
/// account used by the application. It is also scoped by Amazon Web Services Region"
/// (`CreateStream`) — so the same name in two regions is two streams, and only the region
/// that created one resolves it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RegionalName {
    pub region: String,
    pub name: String,
}

impl RegionalName {
    /// The key `name` occupies in `region`.
    pub fn new(region: &str, name: &str) -> Self {
        Self {
            region: region.to_string(),
            name: name.to_string(),
        }
    }
}

/// One data record of a shard — "the unit of data of the Kinesis data stream, which is
/// composed of a sequence number, a partition key, and a data blob" (`Record`) — as a
/// write stored it and a read answers it.
#[derive(Debug, Clone)]
pub struct DataRecord {
    /// `Record$SequenceNumber` — "The unique identifier of the record within its shard",
    /// as [`record_sequence_number`] mints it.
    pub sequence_number: String,
    /// `Record$ApproximateArrivalTimestamp` — "The approximate time that the record was
    /// inserted into the stream", which is also what a `AT_TIMESTAMP` reader positions by
    /// and what the retention window is measured against.
    pub arrived_at: SystemTime,
    /// `Record$PartitionKey` — "Identifies which shard in the stream the data record is
    /// assigned to". `None` for a record written to an `AUTO` stream without one, which is
    /// the one case the member documents as absent.
    pub partition_key: Option<String>,
    /// `Record$Data` — "The data blob. The data in the blob is both opaque and immutable
    /// to Kinesis Data Streams, which does not inspect, interpret, or change the data in
    /// the blob in any way", so it is stored and answered byte for byte.
    pub data: Vec<u8>,
    /// `Record$EncryptionType` — "The encryption type used on the record", which is the
    /// stream's own at the instant the record was written.
    pub encryption_type: String,
}

impl DataRecord {
    /// This record as the modelled `Record` shape.
    pub fn to_shape(&self) -> Record {
        Record {
            sequence_number: self.sequence_number.clone(),
            approximate_arrival_timestamp: Some(timestamp(self.arrived_at)),
            data: self.data.clone(),
            partition_key: self.partition_key.clone(),
            encryption_type: Some(self.encryption_type.clone()),
        }
    }

    /// The bytes the record occupies in the store, which is what
    /// [`MAX_RECORD_BYTES_PER_SHARD`] bounds and what one `GetRecords` page is measured
    /// against ([`MAX_GET_RECORDS_BYTES`]).
    ///
    /// Both halves of the record count, the same way a write measures what it may store:
    /// "When the data blob (the payload before base64-encoding) is added to the partition
    /// key size, the total size must not exceed the maximum record size"
    /// (`PutRecordInput$Data`).
    pub fn stored_bytes(&self) -> usize {
        self.data.len()
            + self
                .partition_key
                .as_deref()
                .map(str::len)
                .unwrap_or_default()
    }
}

/// One shard of a stream's shard map: the slice of the hash key space it covers, the
/// sequence numbers it spans, and the data records written to it.
#[derive(Debug, Clone)]
pub struct ShardRecord {
    /// The shard's id, as [`shard_id`] mints it.
    pub shard_id: String,
    /// Decimal `StartingHashKey`, inside the `HashKey` pattern `^(0|([1-9]\d{0,38}))$`.
    pub starting_hash_key: String,
    /// Decimal `EndingHashKey`.
    pub ending_hash_key: String,
    /// The first sequence number the shard can hold.
    pub starting_sequence_number: String,
    /// The last one it can hold. `None` while the shard is open — "Shards that are in the
    /// OPEN state have an ending sequence number of null"
    /// (`SequenceNumberRange$EndingSequenceNumber`).
    pub ending_sequence_number: Option<String>,
    /// When the shard was opened: the instant the create or the reshard that laid it down
    /// ran. A shard filter selects by it, so it is stored rather than derived.
    pub created_at: SystemTime,
    /// When the shard was closed, `None` while it is open. A reshard is what closes a
    /// shard, and it sets this instant and [`Self::ending_sequence_number`] together.
    pub closed_at: Option<SystemTime>,
    /// `Shard$ParentShardId` — "The shard ID of the shard's parent": the shard this one
    /// was produced from by a split or a merge. `None` for a shard a create laid down.
    pub parent_shard_id: Option<String>,
    /// `Shard$AdjacentParentShardId` — "The shard ID of the shard adjacent to the shard's
    /// parent": the second shard this one was produced from, which only a merge has.
    pub adjacent_parent_shard_id: Option<String>,
    /// The records written to the shard, oldest first — the order `GetRecords` reads them
    /// in ("the shard iterator specifies the position in the shard from which you want to
    /// start reading data records sequentially").
    pub records: Vec<DataRecord>,
    /// How many records have ever been written to the shard, which is what the next
    /// sequence number is minted from ([`record_sequence_number`]). Counted rather than
    /// taken from the stored records, so a number a trim or the store's ceiling released
    /// is never handed out twice.
    pub records_written: u64,
    /// How many bytes [`Self::records`] occupies, maintained as records are appended and
    /// dropped so the write path never re-sums the whole shard.
    pub records_bytes: usize,
}

impl ShardRecord {
    /// A brand-new OPEN shard covering `starting_hash_key`..`ending_hash_key`, holding no
    /// records: what a create lays down.
    pub fn open_at(
        index: u32,
        created_at: SystemTime,
        starting_hash_key: String,
        ending_hash_key: String,
    ) -> Self {
        Self {
            shard_id: shard_id(index),
            starting_hash_key,
            ending_hash_key,
            starting_sequence_number: starting_sequence_number(created_at, index),
            // A shard a create laid down is OPEN, so it has no ending sequence number and
            // no close instant.
            ending_sequence_number: None,
            created_at,
            closed_at: None,
            // A shard a create laid down came from no split and no merge, so it names no
            // parent; [`Self::from_parents`] is what a reshard lays a child down with.
            parent_shard_id: None,
            adjacent_parent_shard_id: None,
            records: Vec::new(),
            records_written: 0,
            records_bytes: 0,
        }
    }

    /// The same brand-new OPEN shard, produced from the shards named in `parents` — the
    /// open shards whose hash key ranges the new one covers.
    ///
    /// "The shard ID of the shard's parent" and "the shard ID of the shard adjacent to the
    /// shard's parent" (`Shard`): a child of ONE shard is a split's, a child of two is a
    /// merge's. A re-tiling that covers more than two of them reaches the child through
    /// short-lived intermediate shards — "this can cause short-lived shards to be created,
    /// in addition to the final shards" (`UpdateShardCount`) — none of which this emulator
    /// lays down, so the lineage it records names the lowest and the highest of the shards
    /// the child covers.
    pub fn from_parents(
        index: u32,
        created_at: SystemTime,
        starting_hash_key: String,
        ending_hash_key: String,
        parents: &[String],
    ) -> Self {
        Self {
            parent_shard_id: parents.first().cloned(),
            // One parent is a split's child: it has a parent and no adjacent parent.
            adjacent_parent_shard_id: parents.last().filter(|_| parents.len() > 1).cloned(),
            ..Self::open_at(index, created_at, starting_hash_key, ending_hash_key)
        }
    }

    /// Close the shard at `at`, which is what a reshard does to every shard it re-tiles:
    /// the shard stops being open, so it takes the ending sequence number its write
    /// counter has reached — "shards that are in the OPEN state have an ending sequence
    /// number of null" (`SequenceNumberRange$EndingSequenceNumber`), and a closed one does
    /// not. The records it holds stay readable until the retention window releases them.
    pub fn close_at(&mut self, at: SystemTime) {
        self.ending_sequence_number = Some(record_sequence_number(
            &self.starting_sequence_number,
            self.records_written,
        ));
        self.closed_at = Some(at);
    }

    /// Whether `shard_id` is one of the shards this one was produced from.
    pub fn is_child_of(&self, shard_id: &str) -> bool {
        [
            self.parent_shard_id.as_deref(),
            self.adjacent_parent_shard_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|parent| parent == shard_id)
    }

    /// The shards this one was produced from, as `ChildShard$ParentShards` reports them.
    pub fn parent_shard_ids(&self) -> Vec<String> {
        [
            self.parent_shard_id.clone(),
            self.adjacent_parent_shard_id.clone(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// The slice of the hash key space this shard covers, as a pair of integers. A bound
    /// this emulator did not write — one that does not parse — covers nothing.
    pub fn hash_key_bounds(&self) -> Option<(u128, u128)> {
        Some((
            self.starting_hash_key.parse::<u128>().ok()?,
            self.ending_hash_key.parse::<u128>().ok()?,
        ))
    }

    /// Append `record` to the shard and answer the sequence number it was stored under.
    ///
    /// The number comes from the shard's own write counter, so it is unique within the
    /// shard and orders the records the way they were written — which is the order a
    /// reader reads them in.
    pub fn append(
        &mut self,
        partition_key: Option<String>,
        data: Vec<u8>,
        encryption_type: &str,
        arrived_at: SystemTime,
    ) -> String {
        let sequence_number =
            record_sequence_number(&self.starting_sequence_number, self.records_written);
        self.records_written = self.records_written.saturating_add(1);
        let record = DataRecord {
            sequence_number: sequence_number.clone(),
            arrived_at,
            partition_key,
            data,
            encryption_type: encryption_type.to_string(),
        };
        self.records_bytes = self.records_bytes.saturating_add(record.stored_bytes());
        self.records.push(record);
        self.enforce_storage_ceiling();
        sequence_number
    }

    /// Hold the shard inside the emulator's own ceilings ([`MAX_RECORDS_PER_SHARD`] /
    /// [`MAX_RECORD_BYTES_PER_SHARD`]) by dropping its OLDEST records, which is the end a
    /// retention window releases too.
    ///
    /// The NEWEST record is never dropped: a write answers the sequence number it stored a
    /// record under, so that record has to be there for the reader that is handed it.
    fn enforce_storage_ceiling(&mut self) {
        let droppable = self.records.len().saturating_sub(1);
        let mut dropped = 0usize;
        while dropped < droppable {
            if self.records.len() - dropped <= MAX_RECORDS_PER_SHARD
                && self.records_bytes <= MAX_RECORD_BYTES_PER_SHARD
            {
                break;
            }
            let Some(record) = self.records.get(dropped) else {
                break;
            };
            self.records_bytes = self.records_bytes.saturating_sub(record.stored_bytes());
            dropped += 1;
        }
        if dropped > 0 {
            self.records.drain(..dropped);
            // The emulator's own ceiling, not the stream's retention window, released these
            // records: a reader sees a shorter shard than the window would have kept, so the
            // difference is diagnosable rather than invisible.
            ::tracing::debug!(
                shard_id = %self.shard_id,
                dropped,
                retained = self.records.len(),
                records_bytes = self.records_bytes,
                "dropped the shard's oldest records at the emulator's per-shard ceiling"
            );
        }
    }

    /// The records the shard still retains at `trim_horizon`: "a stream's data records are
    /// accessible for 24 hours from the time they are added to the stream" and for the
    /// stream's `RetentionPeriodHours` once that window is changed, so a record that
    /// arrived before the horizon is trimmed rather than read.
    pub fn retained(&self, trim_horizon: SystemTime) -> impl DoubleEndedIterator<Item = &DataRecord> {
        self.records
            .iter()
            .filter(move |record| record.arrived_at >= trim_horizon)
    }

    /// This record as the modelled `Shard` shape.
    pub fn to_shape(&self) -> Shard {
        Shard {
            shard_id: self.shard_id.clone(),
            // Both are set only on a shard produced by a split or a merge, which is what a
            // reshard performs ([`Self::from_parents`]); a shard a create laid down has
            // neither.
            parent_shard_id: self.parent_shard_id.clone(),
            adjacent_parent_shard_id: self.adjacent_parent_shard_id.clone(),
            hash_key_range: HashKeyRange {
                starting_hash_key: self.starting_hash_key.clone(),
                ending_hash_key: self.ending_hash_key.clone(),
            },
            sequence_number_range: SequenceNumberRange {
                starting_sequence_number: self.starting_sequence_number.clone(),
                ending_sequence_number: self.ending_sequence_number.clone(),
            },
        }
    }

    /// Whether the shard is still open, so it counts towards `OpenShardCount`.
    pub fn is_open(&self) -> bool {
        self.ending_sequence_number.is_none() && self.closed_at.is_none()
    }

    /// Whether the shard was open at `instant`: it had been laid down by then and nothing
    /// had closed it yet.
    pub fn was_open_at(&self, instant: SystemTime) -> bool {
        self.created_at <= instant && self.closed_at.is_none_or(|closed| closed > instant)
    }

    /// Whether the shard still held data at `instant`: it was laid down no later than
    /// `instant` and, if it has been closed, closed no earlier.
    pub fn spans(&self, instant: SystemTime) -> bool {
        self.created_at <= instant && self.closed_at.is_none_or(|closed| closed >= instant)
    }

    /// Whether the shard is open or was closed no earlier than `instant`.
    pub fn open_or_closed_since(&self, instant: SystemTime) -> bool {
        self.closed_at.is_none_or(|closed| closed >= instant)
    }

    /// Whether `hash_key` falls inside the slice of the hash key space this shard covers
    /// (`Shard$HashKeyRange`, "The range of possible hash key values for the shard, which
    /// is a set of ordered contiguous positive integers"). A range this emulator did not
    /// write — one whose bounds do not parse — covers nothing rather than everything.
    pub fn covers(&self, hash_key: u128) -> bool {
        let Ok(starting) = self.starting_hash_key.parse::<u128>() else {
            return false;
        };
        let Ok(ending) = self.ending_hash_key.parse::<u128>() else {
            return false;
        };
        (starting..=ending).contains(&hash_key)
    }
}

/// One stored Kinesis data stream: the authoritative record every stream operation reads.
/// `CreateStream` inserts it and `DescribeStream` / `DescribeStreamSummary` project it
/// onto their output shapes.
#[derive(Debug, Clone)]
pub struct Stream {
    /// The `StreamName` the create carried, which is half of this record's key.
    pub name: String,
    /// The region the creating request was signed for.
    pub region: String,
    /// The `StreamARN`, as [`stream_arn`] builds it.
    pub arn: String,
    /// `StreamMode` enum value — `PROVISIONED` or `ON_DEMAND`.
    pub stream_mode: String,
    /// The stream's shard map, lowest hash key first.
    pub shards: Vec<ShardRecord>,
    /// `RetentionPeriodHours`.
    pub retention_period_hours: i32,
    /// When `CreateStream` minted the stream.
    pub created_at: SystemTime,
    /// When the stream's `CREATING` -> `ACTIVE` transition falls due: "Upon receiving a
    /// CreateStream request, Kinesis Data Streams immediately returns and sets the stream
    /// status to CREATING. After the stream is created, Kinesis Data Streams sets the
    /// stream status to ACTIVE" (`CreateStream`).
    pub active_at: SystemTime,
    /// `EncryptionType` enum value.
    pub encryption_type: String,
    /// The KMS key encryption uses, set only once a stream is encrypted.
    pub key_id: Option<String>,
    /// The shard-level metrics enhanced monitoring reports. Empty until
    /// `EnableEnhancedMonitoring` names some.
    pub shard_level_metrics: Vec<String>,
    /// `WarmThroughputMiBps`, when the create asked for a target warm throughput.
    pub warm_throughput_mi_bps: Option<i32>,
    /// `MaxRecordSizeInKiB`, when the create named one.
    pub max_record_size_in_ki_b: Option<i32>,
    /// `RecordDistributionStrategy`, which only an on-demand stream carries.
    pub record_distribution_strategy: Option<String>,
    /// When the stream's `DELETING` -> gone transition falls due, set by the
    /// `DeleteStream` that accepted the removal: "After a DeleteStream request, the
    /// specified stream is in the DELETING state until Kinesis Data Streams completes the
    /// deletion" (`DeleteStream`). `None` for a stream nothing has deleted.
    pub deleted_at: Option<SystemTime>,
    /// When the stream's `UPDATING` -> `ACTIVE` transition falls due, set by the
    /// shard-count change that accepted the re-tiling: "Upon receiving the request...
    /// sets the status of the stream to UPDATING. After the update is complete, Kinesis
    /// Data Streams sets the status of the stream back to ACTIVE" (`UpdateShardCount`).
    /// `None` for a stream nothing has rescaled.
    pub updating_until: Option<SystemTime>,
    /// The instants the stream's shard count was changed at, youngest last — what the
    /// documented "ten times per rolling 24-hour period per stream" ceiling
    /// ([`MAX_RESHARDS_PER_WINDOW`]) is counted over. Pruned to [`RESHARD_WINDOW`] by the
    /// change that counts it, so it holds at most that many instants.
    pub reshards: Vec<SystemTime>,
    /// The tags the create carried.
    pub tags: HashMap<String, String>,
}

impl Stream {
    /// The strategy that places a record written to this stream: the one the create named,
    /// or the documented default — "If the stream uses the USER_PARTITION_KEY record
    /// distribution strategy (the default), a partition key is required"
    /// (`PutRecordInput$PartitionKey`).
    pub fn record_distribution(&self) -> &str {
        self.record_distribution_strategy
            .as_deref()
            .unwrap_or(RECORD_DISTRIBUTION_USER_PARTITION_KEY)
    }

    /// Which shard of the map a record carrying `explicit_hash_key` / `partition_key` is
    /// assigned to, by the index it occupies in [`Self::shards`].
    ///
    /// "Amazon Kinesis Data Streams uses the partition key as input to a hash function that
    /// maps the partition key and associated data to a specific shard... all data records
    /// with the same partition key map to the same shard within the stream"
    /// (`PutRecordInput$PartitionKey`), and `ExplicitHashKey` names that hash value
    /// directly ("The hash value used to explicitly determine the shard the data record is
    /// assigned to"). Under the `AUTO` strategy "the partition key is optional and any
    /// value you provide is ignored, along with any ExplicitHashKey you provide... Amazon
    /// Kinesis Data Streams distributes the record across shards using service-managed
    /// algorithms", so the record goes to the open shard the stream's write count selects,
    /// advanced by `nth_in_call` — the record's position within the call being served — so
    /// that one batch spreads over every open shard rather than landing in one.
    pub fn placement(
        &self,
        explicit_hash_key: Option<&str>,
        partition_key: Option<&str>,
        nth_in_call: u64,
    ) -> Result<usize, RecordPlacementFailure> {
        let open: Vec<usize> = self
            .shards
            .iter()
            .enumerate()
            .filter(|(_, shard)| shard.is_open())
            .map(|(index, _)| index)
            .collect();
        if open.is_empty() {
            return Err(RecordPlacementFailure::NoOpenShard);
        }
        if self.record_distribution() == RECORD_DISTRIBUTION_AUTO {
            let written: u64 = self
                .shards
                .iter()
                .fold(0u64, |total, shard| total.saturating_add(shard.records_written));
            let position = usize::try_from(written.wrapping_add(nth_in_call) % open.len() as u64)
                .unwrap_or(0);
            return open
                .get(position)
                .copied()
                .ok_or(RecordPlacementFailure::NoOpenShard);
        }
        let hash = match explicit_hash_key {
            // `ExplicitHashKey` is modelled `^(0|([1-9]\d{0,38}))$`, which admits values
            // past `u128::MAX`; one of those lands in no shard's range, which is the same
            // refusal as a value between two ranges.
            Some(explicit) => explicit
                .parse::<u128>()
                .map_err(|_| RecordPlacementFailure::OutsideTheShardMap(explicit.to_string()))?,
            None => {
                let Some(partition_key) = partition_key else {
                    return Err(RecordPlacementFailure::PartitionKeyRequired);
                };
                partition_key_hash(partition_key)
            }
        };
        open.into_iter()
            .find(|index| {
                self.shards
                    .get(*index)
                    .is_some_and(|shard| shard.covers(hash))
            })
            .ok_or_else(|| RecordPlacementFailure::OutsideTheShardMap(hash.to_string()))
    }

    /// The most bytes one record written to this stream may carry: the stream's own
    /// `MaxRecordSizeInKiB` when the create named one, else the documented default — "Each
    /// data record can be up to 1 MiB in size by default. Amazon Kinesis Data Streams
    /// supports large records up to 10 MiB in size" (`GetRecords`), the larger sizes being
    /// what `MaxRecordSizeInKiB` opts into.
    pub fn max_record_size_bytes(&self) -> usize {
        let ki_b = self
            .max_record_size_in_ki_b
            .unwrap_or(DEFAULT_MAX_RECORD_SIZE_KI_B)
            .max(1);
        usize::try_from(ki_b).unwrap_or(usize::MAX).saturating_mul(1024)
    }

    /// Append one record to the shard at `index` in the shard map — the index
    /// [`Self::placement`] resolved for it — answering that shard's id and the sequence
    /// number the record was stored under, or `None` for an index the map does not hold.
    ///
    /// ONE path for every write: `PutRecord` and `PutRecords` resolve the shard by the same
    /// rule ([`Self::placement`]) and store through this append, so a reader cannot tell
    /// which operation wrote a record.
    ///
    /// The partition key is stored as the producer sent it: under the `AUTO` strategy "the
    /// partition key is optional and any value you provide is ignored", but
    /// `Record$PartitionKey` is still "the original value... even though it was not used to
    /// determine shard placement".
    pub fn append_at(
        &mut self,
        index: usize,
        partition_key: Option<String>,
        data: Vec<u8>,
        at: SystemTime,
    ) -> Option<(String, String)> {
        let encryption_type = self.encryption_type.clone();
        let shard = self.shards.get_mut(index)?;
        let sequence_number = shard.append(partition_key, data, &encryption_type, at);
        Some((shard.shard_id.clone(), sequence_number))
    }

    /// The stream's status at `now`: `DELETING` once a delete has been accepted for it,
    /// `CREATING` until the transition the create armed falls due, `UPDATING` until the
    /// one a shard-count change armed falls due, `ACTIVE` from then on. Derived from the
    /// stored instants, so two reads at the same instant agree and no read has to write.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if self.deleted_at.is_some() {
            STREAM_DELETING
        } else if now < self.active_at {
            STREAM_CREATING
        } else if self.updating_until.is_some_and(|due| now < due) {
            STREAM_UPDATING
        } else {
            STREAM_ACTIVE
        }
    }

    /// Whether the stream serves its shard map and its records at `now`.
    ///
    /// "You can continue to read and write data to your stream while its status is
    /// UPDATING" (`UpdateShardCount`), so a stream whose re-tiling is still due serves a
    /// read and a write exactly as an `ACTIVE` stream does; a stream whose create or whose
    /// removal is still due has no shard map to serve at all.
    pub fn serves_shards(&self, now: SystemTime) -> bool {
        matches!(self.status(now), STREAM_ACTIVE | STREAM_UPDATING)
    }

    /// How many shard-count changes the stream has been scaled by inside the window
    /// ending at `now`, which is what [`MAX_RESHARDS_PER_WINDOW`] bounds.
    pub fn reshards_in_window(&self, now: SystemTime) -> usize {
        let from = now.checked_sub(RESHARD_WINDOW).unwrap_or(UNIX_EPOCH);
        self.reshards.iter().filter(|at| **at >= from).count()
    }

    /// Re-tile the stream's open shards onto `target_shard_count` equal slices of the hash
    /// key space at `at`, answering how many shards were open before.
    ///
    /// "To update the shard count, Kinesis Data Streams performs splits or merges on
    /// individual shards" (`UpdateShardCount`), and a uniform scaling "creates shards of
    /// equal size" (`ScalingType`): every shard that was open is closed, and the whole key
    /// space is covered again by `target_shard_count` contiguous equal slices, each child
    /// naming the shards it was produced from. A shard that is already closed is left as it
    /// is, so the records it holds stay readable until the retention window releases them,
    /// and a map that already tiles the key space the target way is left untouched —
    /// nothing is split or merged to reach a tiling it already has.
    ///
    /// Bounded by the caller: a target past the shard ceilings is refused BEFORE this
    /// allocates from it.
    pub fn reshard_uniformly(&mut self, target_shard_count: u32, at: SystemTime) -> i32 {
        let open_before = self.open_shard_count();
        let target = hash_key_ranges(target_shard_count);
        let parents: Vec<(u128, u128, String)> = self
            .shards
            .iter()
            .filter(|shard| shard.is_open())
            .filter_map(|shard| {
                let (starting, ending) = shard.hash_key_bounds()?;
                Some((starting, ending, shard.shard_id.clone()))
            })
            .collect();

        // The shard count is being changed, so the stream owes the `ACTIVE` it goes back
        // to, and this change counts towards the rolling-window ceiling.
        self.updating_until = Some(at + RESHARD_DELAY);
        let from = at.checked_sub(RESHARD_WINDOW).unwrap_or(UNIX_EPOCH);
        self.reshards.retain(|instant| *instant >= from);
        self.reshards.push(at);

        // A map whose open shards already are the target tiling needs no split and no
        // merge: the shards stay open, keeping the sequence numbers their readers hold.
        let unchanged = parents.len() == target.len()
            && target.iter().zip(parents.iter()).all(
                |((starting, ending), (parent_starting, parent_ending, _))| {
                    starting.parse::<u128>().ok() == Some(*parent_starting)
                        && ending.parse::<u128>().ok() == Some(*parent_ending)
                },
            );
        if unchanged {
            return open_before;
        }

        for shard in self.shards.iter_mut().filter(|shard| shard.is_open()) {
            shard.close_at(at);
        }
        // A shard id is the shard's position in the map, so the children take the
        // positions after every shard the map already holds and no id is ever reused.
        let first_index = u32::try_from(self.shards.len()).unwrap_or(u32::MAX);
        for (offset, (starting_hash_key, ending_hash_key)) in target.into_iter().enumerate() {
            let index = first_index.saturating_add(u32::try_from(offset).unwrap_or(u32::MAX));
            let covered: Vec<String> = match (
                starting_hash_key.parse::<u128>(),
                ending_hash_key.parse::<u128>(),
            ) {
                (Ok(starting), Ok(ending)) => parents
                    .iter()
                    .filter(|(parent_starting, parent_ending, _)| {
                        *parent_starting <= ending && *parent_ending >= starting
                    })
                    .map(|(_, _, shard_id)| shard_id.clone())
                    .collect(),
                _ => Vec::new(),
            };
            self.shards.push(ShardRecord::from_parents(
                index,
                at,
                starting_hash_key,
                ending_hash_key,
                &covered,
            ));
        }
        open_before
    }

    /// The children of the shard `shard_id`, as `GetRecords` reports them — "the existing
    /// child shard of the current shard" (`ChildShard`), which is what a reader that has
    /// reached the end of a closed shard reads on from.
    pub fn child_shards_of(&self, shard_id: &str) -> Vec<ChildShard> {
        self.shards
            .iter()
            .filter(|shard| shard.is_child_of(shard_id))
            .map(|shard| ChildShard {
                shard_id: shard.shard_id.clone(),
                parent_shards: shard.parent_shard_ids(),
                hash_key_range: HashKeyRange {
                    starting_hash_key: shard.starting_hash_key.clone(),
                    ending_hash_key: shard.ending_hash_key.clone(),
                },
            })
            .collect()
    }

    /// Whether this account and region still hold the stream at `now`: a stream a delete
    /// armed is held, in `DELETING`, until the removal that delete owes falls due, and
    /// names no resource from then on. Derived from the stored instant, so a reader reaches
    /// the same answer as a writer without either of them having to prune the store.
    pub fn is_held(&self, now: SystemTime) -> bool {
        self.deleted_at.is_none_or(|due| now < due)
    }

    /// The stream's trim horizon at `now`: the oldest instant data is still retained for.
    ///
    /// "A stream's data records are accessible for 24 hours from the time they are added to
    /// the stream" by default, and for `RetentionPeriodHours` once that window is changed,
    /// so the horizon trails `now` by the stream's own retention window — never reaching
    /// back past the instant the stream was created, since nothing was retained before it.
    pub fn trim_horizon(&self, now: SystemTime) -> SystemTime {
        let hours = u64::try_from(self.retention_period_hours).unwrap_or_default();
        let window = Duration::from_secs(hours.saturating_mul(SECONDS_PER_HOUR));
        now.checked_sub(window)
            .filter(|horizon| *horizon > self.created_at)
            .unwrap_or(self.created_at)
    }

    /// How many of the stream's shards are open.
    pub fn open_shard_count(&self) -> i32 {
        i32::try_from(self.shards.iter().filter(|shard| shard.is_open()).count())
            .unwrap_or(i32::MAX)
    }

    /// This record as the modelled `StreamSummary`, which is what a listing reports beside
    /// the stream's name.
    pub fn to_stream_summary(&self, now: SystemTime) -> StreamSummary {
        StreamSummary {
            stream_name: self.name.clone(),
            stream_arn: self.arn.clone(),
            stream_status: self.status(now).to_string(),
            stream_mode_details: Some(self.stream_mode_details()),
            stream_creation_timestamp: Some(timestamp(self.created_at)),
        }
    }

    /// The stream's tags as the modelled `Tag` list, ordered by key.
    ///
    /// A tag list is paged through by key ("The key to use as the starting point for the
    /// list of tags... gets all tags that occur after ExclusiveStartTagKey",
    /// `ListTagsForStreamInput$ExclusiveStartTagKey`), so the tags a stream reports need
    /// ONE total order that every read agrees on, and the cursor names the key: the order
    /// is by key.
    pub fn tags_ordered(&self) -> Vec<Tag> {
        let mut tags: Vec<Tag> = self
            .tags
            .iter()
            .map(|(key, value)| Tag {
                key: key.clone(),
                // `Tag$Value` is "An optional string", and a tag assigned a value carries
                // it however short it is — `TagValue` is modelled `len 0..256`.
                value: Some(value.clone()),
            })
            .collect();
        tags.sort_by(|left, right| left.key.cmp(&right.key));
        tags
    }

    /// How many tags the stream would carry once `tags` were assigned to it, which is what
    /// [`MAX_STREAM_TAGS`] bounds: an assignment overwrites a key the stream already
    /// carries rather than adding a second entry for it.
    pub fn tag_count_with(&self, tags: &HashMap<String, String>) -> usize {
        self.tags.len()
            + tags
                .keys()
                .filter(|key| !self.tags.contains_key(key.as_str()))
                .count()
    }

    /// Assign `tags` to the stream: a key the stream already carries takes the new value
    /// ("If tags have already been assigned to the stream, AddTagsToStream overwrites any
    /// existing tags that correspond to the specified tag keys"), and every tag the
    /// request does not name stays as it was.
    ///
    /// Bounded by the caller: a request that would take the stream past
    /// [`MAX_STREAM_TAGS`] is refused BEFORE this grows the stored map
    /// ([`Self::tag_count_with`]).
    pub fn assign_tags(&mut self, tags: impl IntoIterator<Item = (String, String)>) {
        self.tags.extend(tags);
    }

    /// The enhanced monitoring configuration the stream reports: one entry carrying the
    /// shard-level metrics that are enabled, which is an empty list until some are.
    fn enhanced_monitoring(&self) -> Vec<EnhancedMetrics> {
        vec![EnhancedMetrics {
            shard_level_metrics: Some(self.shard_level_metrics.clone()),
        }]
    }

    /// The warm throughput the stream reports: the target the create asked for, or the
    /// throughput its open shards provide ([`SHARD_WRITE_MI_BPS`] each). Both members
    /// carry the same value — the input member's doc calls it "the throughput capacity
    /// that will be immediately available for write operations".
    fn warm_throughput(&self) -> WarmThroughputObject {
        let target = self
            .warm_throughput_mi_bps
            .unwrap_or_else(|| self.open_shard_count().saturating_mul(SHARD_WRITE_MI_BPS));
        WarmThroughputObject {
            target_mi_bps: Some(target),
            current_mi_bps: Some(target),
        }
    }

    /// The `StreamModeDetails` the stream reports.
    fn stream_mode_details(&self) -> StreamModeDetails {
        StreamModeDetails {
            stream_mode: self.stream_mode.clone(),
        }
    }

    /// This record as the modelled `StreamDescription`, carrying `shards` and whether the
    /// shard map holds more beyond them.
    pub fn to_description(
        &self,
        now: SystemTime,
        shards: Vec<Shard>,
        has_more_shards: bool,
    ) -> StreamDescription {
        StreamDescription {
            stream_name: self.name.clone(),
            stream_arn: self.arn.clone(),
            stream_status: self.status(now).to_string(),
            stream_mode_details: Some(self.stream_mode_details()),
            shards,
            has_more_shards,
            retention_period_hours: self.retention_period_hours,
            stream_creation_timestamp: timestamp(self.created_at),
            enhanced_monitoring: self.enhanced_monitoring(),
            encryption_type: Some(self.encryption_type.clone()),
            key_id: self.key_id.clone(),
        }
    }

    /// This record as the modelled `StreamDescriptionSummary`.
    pub fn to_summary(&self, now: SystemTime) -> StreamDescriptionSummary {
        StreamDescriptionSummary {
            stream_name: self.name.clone(),
            stream_arn: self.arn.clone(),
            // `StreamDescriptionSummary$StreamId` is documented "Not Implemented.
            // Reserved for future use", so no stream carries one.
            stream_id: None,
            stream_status: self.status(now).to_string(),
            stream_mode_details: Some(self.stream_mode_details()),
            retention_period_hours: self.retention_period_hours,
            stream_creation_timestamp: timestamp(self.created_at),
            enhanced_monitoring: self.enhanced_monitoring(),
            encryption_type: Some(self.encryption_type.clone()),
            key_id: self.key_id.clone(),
            open_shard_count: self.open_shard_count(),
            // No consumer is registered until `RegisterStreamConsumer` registers one, and
            // no channel is associated until `CreateChannel` creates one.
            consumer_count: Some(0),
            warm_throughput: Some(self.warm_throughput()),
            max_record_size_in_ki_b: self.max_record_size_in_ki_b,
            channel_count: Some(0),
            record_distribution_strategy: self.record_distribution_strategy.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-kinesis-kinesis-20131202. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKinesisKinesis20131202State {
    /// Every stream this emulator holds, keyed by region and name.
    pub streams: HashMap<RegionalName, Stream>,
}

impl ComAmazonawsKinesisKinesis20131202State {
    /// The stream `region`'s endpoint holds under `name` at `now`, if any.
    pub fn stream_in(&self, now: SystemTime, region: &str, name: &str) -> Option<&Stream> {
        self.streams
            .get(&RegionalName::new(region, name))
            .filter(|stream| stream.is_held(now))
    }

    /// The stream `region`'s endpoint holds under `arn` at `now`, if any.
    pub fn stream_by_arn(&self, now: SystemTime, region: &str, arn: &str) -> Option<&Stream> {
        self.streams
            .values()
            .find(|stream| stream.region == region && stream.arn == arn)
            .filter(|stream| stream.is_held(now))
    }

    /// Every stream `region`'s endpoint holds at `now`, ordered by name.
    ///
    /// A listing is paged by name ("you can request more streams by using the name of the
    /// last stream returned by the ListStreams request in the ExclusiveStartStreamName
    /// parameter", `ListStreams`), so the streams a read reports need ONE total order that
    /// every page agrees on, and the cursor names the stream: the order is by name.
    pub fn streams_ordered_in(&self, now: SystemTime, region: &str) -> Vec<&Stream> {
        let mut streams: Vec<&Stream> = self
            .streams
            .values()
            .filter(|stream| stream.region == region && stream.is_held(now))
            .collect();
        streams.sort_by(|left, right| left.name.cmp(&right.name));
        streams
    }

    /// The stream a request's `StreamName` / `StreamARN` pair names at `now`.
    ///
    /// "When invoking this API, you must use either the StreamARN or the StreamName
    /// parameter, or both" (`DescribeStream`), and that holds for every operation of this
    /// service that addresses a stream: a request naming neither is refused rather than
    /// answered, and a request naming both has to name ONE stream with them.
    pub fn resolve_stream(
        &self,
        now: SystemTime,
        region: &str,
        name: Option<&str>,
        arn: Option<&str>,
    ) -> Result<&Stream, StreamLookupFailure> {
        match (name, arn) {
            (None, None) => Err(StreamLookupFailure::Unspecified),
            (Some(name), None) => self
                .stream_in(now, region, name)
                .ok_or(StreamLookupFailure::NotFound),
            (None, Some(arn)) => self
                .stream_by_arn(now, region, arn)
                .ok_or(StreamLookupFailure::NotFound),
            (Some(name), Some(arn)) => {
                let stream = self
                    .stream_by_arn(now, region, arn)
                    .ok_or(StreamLookupFailure::NotFound)?;
                if stream.name == name {
                    Ok(stream)
                } else {
                    Err(StreamLookupFailure::Mismatch)
                }
            }
        }
    }

    /// The key of the stream a request's `StreamName` / `StreamARN` pair names, resolved
    /// by exactly the rule [`Self::resolve_stream`] applies.
    fn resolve_stream_key(
        &self,
        now: SystemTime,
        region: &str,
        name: Option<&str>,
        arn: Option<&str>,
    ) -> Result<RegionalName, StreamLookupFailure> {
        self.resolve_stream(now, region, name, arn)
            .map(|stream| RegionalName::new(&stream.region, &stream.name))
    }

    /// The stream a request's `StreamName` / `StreamARN` pair names, for an operation that
    /// WRITES to it. One rule, one lookup: the pair is resolved by
    /// [`Self::resolve_stream_key`] and the stored record is then reached by its key.
    pub fn resolve_stream_mut(
        &mut self,
        now: SystemTime,
        region: &str,
        name: Option<&str>,
        arn: Option<&str>,
    ) -> Result<&mut Stream, StreamLookupFailure> {
        let key = self.resolve_stream_key(now, region, name, arn)?;
        self.streams
            .get_mut(&key)
            .ok_or(StreamLookupFailure::NotFound)
    }

    /// Drop every stream whose removal has fallen due at `now`, freeing its shard logs.
    ///
    /// The removal a `DeleteStream` armed is what makes the stream stop naming a resource
    /// ([`Stream::is_held`]), which every lookup decides from the stored instant — so this
    /// changes what no caller sees and only hands the memory back. Called by the operations
    /// that change which streams this account holds, which are the ones whose ceilings are
    /// counted against the store.
    pub fn drop_removed_streams(&mut self, now: SystemTime) {
        self.streams.retain(|_, stream| stream.is_held(now));
    }

    /// How many streams `region`'s endpoint holds at `now`, which is what
    /// [`MAX_STREAMS_PER_REGION`] bounds.
    pub fn streams_held_in(&self, now: SystemTime, region: &str) -> usize {
        self.streams
            .iter()
            .filter(|(key, stream)| key.region == region && stream.is_held(now))
            .count()
    }

    /// How many shards `region`'s endpoint holds at `now` across every stream, which is
    /// what [`MAX_SHARDS_PER_REGION`] bounds.
    pub fn shards_held_in(&self, now: SystemTime, region: &str) -> u32 {
        self.streams
            .values()
            .filter(|stream| stream.region == region && stream.is_held(now))
            .fold(0u32, |held, stream| {
                held.saturating_add(u32::try_from(stream.shards.len()).unwrap_or(u32::MAX))
            })
    }

    /// How many streams this emulator holds at `now` across every region, which is what
    /// [`MAX_STREAMS_TOTAL`] bounds: the ceiling no request can partition.
    pub fn streams_held(&self, now: SystemTime) -> usize {
        self.streams
            .values()
            .filter(|stream| stream.is_held(now))
            .count()
    }

    /// How many shards this emulator holds at `now` across every region, which is what
    /// [`MAX_SHARDS_TOTAL`] bounds: the ceiling no request can partition.
    pub fn shards_held(&self, now: SystemTime) -> u32 {
        self.streams
            .values()
            .filter(|stream| stream.is_held(now))
            .fold(0u32, |held, stream| {
                held.saturating_add(u32::try_from(stream.shards.len()).unwrap_or(u32::MAX))
            })
    }
}

/// Why a write places a record in no shard of the stream it addresses. The operation maps
/// it onto the error ITS model declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordPlacementFailure {
    /// The stream's records are placed by the producer's partition key, and the request
    /// carried none ([`PARTITION_KEY_REQUIRED_MESSAGE`]).
    PartitionKeyRequired,
    /// The hash key the request named — explicitly, or through its partition key — lies in
    /// no open shard's range ([`hash_key_outside_the_shard_map_message`]).
    OutsideTheShardMap(String),
    /// Every shard of the stream is closed, so nothing can be written to it.
    NoOpenShard,
}

/// Why a request's `StreamName` / `StreamARN` pair names no stream a store holds. Each
/// operation maps it onto the error ITS model declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamLookupFailure {
    /// The request named neither member, so it names no stream at all.
    Unspecified,
    /// The request named both, and they name different streams.
    Mismatch,
    /// The members are well formed, but this account and region hold no such stream.
    NotFound,
}

/// How a request that names no stream is refused: the rule `DescribeStream` publishes
/// ("you must use either the StreamARN or the StreamName parameter, or both"), which is
/// what makes the refusal part of the contract rather than wording.
pub const STREAM_IDENTIFIER_REQUIRED_MESSAGE: &str =
    "you must use either the StreamARN or the StreamName parameter, or both";

/// How a request whose `StreamName` and `StreamARN` name different streams is refused.
pub const STREAM_IDENTIFIER_MISMATCH_MESSAGE: &str =
    "StreamName and StreamARN name different streams; name one stream or supply only one \
     of them";

/// How a read states that this account and region hold no stream under the identifier the
/// request named. Built FROM the request, so the identifier at fault is what names it.
pub fn stream_not_found_message(identifier: &str) -> String {
    format!(
        "stream {identifier} not found under account {}",
        ::engine::emulator_account_id()
    )
}

/// How an operation that needs a ready stream refuses one that is not ready yet: the rule
/// `ResourceInUseException` publishes ("The resource is not available for this operation.
/// For successful operation, the resource must be in the ACTIVE state"). Built FROM the
/// request and the stored record, so the identifier and the status at fault name it.
pub fn stream_not_active_message(identifier: &str, status: &str) -> String {
    format!(
        "stream {identifier} is in state {status}, not {STREAM_ACTIVE}; the stream must be \
         {STREAM_ACTIVE} for this operation"
    )
}

/// How an operation that only serves a provisioned stream refuses one whose shard count
/// the service manages itself: "specifies that you tried to invoke this API for a data
/// stream with the on-demand capacity mode. This API is only supported for data streams
/// with the provisioned capacity mode" (`ValidationException`). Built FROM the stored
/// record and this account, so the stream at fault names it.
///
/// ONE sentence for the one rule: `UpdateShardCount`, `SplitShard` and `MergeShards` all
/// re-tile the shard map a provisioned stream owns, so they refuse an on-demand stream
/// identically rather than each wording it.
pub fn stream_on_demand_message(stream_name: &str) -> String {
    format!(
        "Request is invalid. Stream {stream_name} under account {} is in On-Demand mode.",
        ::engine::emulator_account_id()
    )
}

/// How an assignment that would leave a resource carrying more tags than one may carry is
/// refused: the ceiling ([`MAX_STREAM_TAGS`]) is on what the resource carries once the
/// request is served, so the refusal states the count the assignment would reach.
///
/// ONE sentence for the one rule: `AddTagsToStream` and `TagResource` are two ways into the
/// SAME stored tag set, so they refuse the ceiling identically rather than each wording it.
pub fn tag_ceiling_exceeded_message(resulting: usize) -> String {
    format!(
        "assigning these tags would leave the resource carrying {resulting} tags, past the \
         {MAX_STREAM_TAGS} tags one resource may carry"
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKinesisKinesis20131202State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKinesisKinesis20131202State>)
}

/// The region a request was signed for, or [`DEFAULT_REGION`] when it carries no SigV4
/// credential scope or one whose region cannot name an AWS region.
///
/// The region is a store key and a quota partition, so a value that is not shaped like a
/// region name is not adopted as either: it falls back to the default region.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_name(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The most characters a region name carries, comfortably past every published AWS region.
const MAX_REGION_NAME_LENGTH: usize = 32;

/// Whether `region` is shaped like an AWS region name — lowercase letters, digits and
/// dashes, bounded in length — so it can serve as a store key and a quota partition.
fn is_region_name(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= MAX_REGION_NAME_LENGTH
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The modelled timestamp for a stored instant.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    match at.duration_since(UNIX_EPOCH) {
        Ok(since) => ::aws_smithy_types::DateTime::from_secs_f64(since.as_secs_f64()),
        Err(_) => ::aws_smithy_types::DateTime::from_secs(0),
    }
}

/// The ARN a stream named `name` in `region` is addressed by, built through the shared
/// `engine` helper so the account segment is the one canonical placeholder account.
pub fn stream_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(SERVICE_ARN_NAMESPACE, region, &format!("stream/{name}"))
}

/// The id of the `index`th shard of a stream: `shardId-` and the index in twelve digits,
/// inside the `ShardId` pattern `^[a-zA-Z0-9_.-]+$`.
pub fn shard_id(index: u32) -> String {
    format!("shardId-{index:012}")
}

/// The hash key range each of `shard_count` shards covers, lowest first.
///
/// "Scale-out within a stream is explicitly supported by means of shards, which are
/// uniquely identified groups of data records in a stream" (`CreateStream`): a stream's
/// shards partition the 128-bit hash key space into contiguous, equal, non-overlapping
/// slices, so the ranges run from 0 to 2^128-1 without a gap.
pub fn hash_key_ranges(shard_count: u32) -> Vec<(String, String)> {
    let shards = u128::from(shard_count.max(1));
    // 2^128 is one past `u128::MAX`, so the quotient and remainder of the division by the
    // shard count are taken against `u128::MAX` and the remainder carried back by one.
    let quotient = u128::MAX / shards;
    let remainder = u128::MAX % shards + 1;
    // The lowest hash key of the `index`th shard: floor(index * 2^128 / shard_count).
    let boundary = |index: u128| index * quotient + index * remainder / shards;
    (0..shards)
        .map(|index| {
            let start = boundary(index);
            let end = if index + 1 == shards {
                u128::MAX
            } else {
                boundary(index + 1) - 1
            };
            (start.to_string(), end.to_string())
        })
        .collect()
}

/// The first sequence number the `index`th shard of a stream created at `created_at` can
/// hold, inside the `SequenceNumber` pattern `^(0|([1-9]\d{0,128}))$`.
///
/// A sequence number is opaque to a client, so this one is DERIVED from the stream's own
/// creation instant (in milliseconds) and the shard's position: two shards of one stream
/// differ, two streams created at different instants differ, and the value a read reports
/// never changes.
pub fn starting_sequence_number(created_at: SystemTime, index: u32) -> String {
    let millis = epoch_millis(created_at);
    format!("49{millis:013}{index:06}")
}

/// How many digits a record's sequence number appends to the shard's starting sequence
/// number. Fixed width, so every sequence number of one shard is the same length and
/// comparing two of them digit by digit is their numeric order — which is also the order
/// the records were written in.
const RECORD_SEQUENCE_NUMBER_DIGITS: usize = 12;

/// The sequence number of the record a shard writes next: "the unique identifier of the
/// record within its shard" (`Record$SequenceNumber`), inside the `SequenceNumber` pattern
/// `^(0|([1-9]\d{0,128}))$`.
///
/// DERIVED from the shard's own starting sequence number and its write counter, so it lies
/// inside the range the shard publishes (`Shard$SequenceNumberRange`), it is larger than
/// every number written before it, and the value a read reports never changes.
pub fn record_sequence_number(shard_starting_sequence_number: &str, written: u64) -> String {
    format!(
        "{shard_starting_sequence_number}{written:0width$}",
        width = RECORD_SEQUENCE_NUMBER_DIGITS
    )
}

/// Whether `left` is a sequence number no larger than `right`.
///
/// `SequenceNumber` is a decimal integer without leading zeros (`^(0|([1-9]\d{0,128}))$`),
/// which is wider than any integer type this crate holds — so the comparison is by digit
/// COUNT first and then lexicographic, which for that form is the numeric order.
pub fn sequence_number_le(left: &str, right: &str) -> bool {
    (left.len(), left) <= (right.len(), right)
}

/// The `RecordDistributionStrategy` under which the producer's partition key places the
/// record: "If the stream uses the USER_PARTITION_KEY record distribution strategy (the
/// default), a partition key is required" (`PutRecordInput$PartitionKey`).
pub const RECORD_DISTRIBUTION_USER_PARTITION_KEY: &str = "USER_PARTITION_KEY";

/// The `RecordDistributionStrategy` under which the service places the record itself: "If
/// the stream uses the AUTO record distribution strategy, the partition key is optional
/// and any value you provide is ignored, along with any ExplicitHashKey you provide"
/// (`PutRecordInput$PartitionKey`).
pub const RECORD_DISTRIBUTION_AUTO: &str = "AUTO";

/// The 128-bit hash key a partition key maps to: "an MD5 hash function is used to map
/// partition keys to 128-bit integer values and to map associated data records to shards.
/// As a result of this hashing mechanism, all data records with the same partition key map
/// to the same shard within the stream" (`PutRecordInput$PartitionKey`).
///
/// The digest is PROTOCOL-MANDATED record placement, not a security primitive: a client
/// computes the same mapping to predict which shard its records land in, so the published
/// `HashKeyRange` of a shard only means anything with this exact function. Nothing here
/// authenticates, signs or protects anything.
pub fn partition_key_hash(partition_key: &str) -> u128 {
    let digest = <::md5::Md5 as ::md5::Digest>::digest(partition_key.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(digest.as_slice());
    u128::from_be_bytes(bytes)
}

/// How a write states that the request's `ExplicitHashKey` or `PartitionKey` lands in no
/// shard of the stream. Built FROM the request, so the value at fault names it.
pub fn hash_key_outside_the_shard_map_message(hash_key: &str) -> String {
    format!(
        "the hash key {hash_key} lies in no open shard of the stream; an ExplicitHashKey \
         must fall inside one shard's HashKeyRange"
    )
}

/// How a write refuses a request that names no `PartitionKey` for a stream whose records a
/// partition key places: the rule `PutRecordInput$PartitionKey` publishes ("If the stream
/// uses the USER_PARTITION_KEY record distribution strategy (the default), a partition key
/// is required").
pub const PARTITION_KEY_REQUIRED_MESSAGE: &str =
    "PartitionKey is required for a stream that uses the USER_PARTITION_KEY record \
     distribution strategy";

/// The maximum size of one data record when the stream names no `MaxRecordSizeInKiB`:
/// "Each data record can be up to 1 MiB in size by default. Amazon Kinesis Data Streams
/// supports large records up to 10 MiB in size" (`GetRecords`) — which is also the
/// smallest value `MaxRecordSizeInKiB` is modelled to take (`range 1024..10240`).
pub const DEFAULT_MAX_RECORD_SIZE_KI_B: i32 = 1_024;

/// The most bytes one `PutRecords` request carries across its entries: "Each PutRecords
/// request can support up to 500 records. Each record in the request can be as large as 1
/// MiB, up to a limit of 5 MiB for the entire request" (`PutRecords`) — the 500 is the
/// modelled `@length(max: 500)` the boundary validator enforces, this is the aggregate it
/// does not.
pub const MAX_PUT_RECORDS_REQUEST_BYTES: usize = 5 * 1024 * 1024;

/// How a write refuses a batch whose records total more than one request may carry
/// ([`MAX_PUT_RECORDS_REQUEST_BYTES`]). Built FROM the request, so the total at fault and
/// the total allowed both name it.
pub fn batch_too_large_message(size: usize, maximum: usize) -> String {
    format!(
        "the records of this request total {size} bytes, past the {maximum} bytes one \
         PutRecords request may carry"
    )
}

/// How a write refuses a record past the stream's maximum record size: "When the data blob
/// (the payload before base64-encoding) is added to the partition key size, the total size
/// must not exceed the maximum record size" (`PutRecordInput$Data`). Built FROM the request
/// and the stream, so the size at fault and the size allowed both name it.
pub fn record_too_large_message(size: usize, maximum: usize) -> String {
    format!(
        "the record's data and partition key total {size} bytes, past the {maximum} bytes \
         one record of this stream may carry"
    )
}

/// What a `ListShards` pagination token is tagged with, so a string the service never
/// issued is refused rather than read as a shard id.
const SHARD_PAGE_TOKEN_TAG: &str = "listShards";

/// Milliseconds since the epoch at `at`, which is how an instant travels inside a
/// pagination token.
pub fn epoch_millis(at: SystemTime) -> u128 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or(0)
}

/// Where a truncated `ListShards` page continues, which stream's shards it continues, and
/// when the page that handed the token out was served.
///
/// The token "unambiguously identifies the stream" (`ListShardsInput$NextToken`), so it
/// carries the stream's ARN AND the instant that stream was created — a name a delete
/// released and a later create took again is another stream, and the pair names only one of
/// them — beside the shard the next page starts at; and it expires, so it carries the
/// instant it was minted. All of it travels INSIDE the token: a read mints one without
/// writing anything a later caller could see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardPageToken {
    /// The ARN of the stream whose shards are being listed.
    pub stream_arn: String,
    /// When that stream was created, in milliseconds since the epoch.
    pub stream_created_at_millis: u128,
    /// The id of the first shard the page did NOT return, which the next page starts at.
    pub next_shard_id: String,
    /// When the page that handed this token out was served, in milliseconds since the
    /// epoch.
    pub issued_at_millis: u128,
}

impl ShardPageToken {
    /// The token that resumes at `next_shard_id` for the stream `stream_arn` created at
    /// `stream_created_at`, minted at `issued_at`.
    pub fn new(
        stream_arn: &str,
        stream_created_at: SystemTime,
        next_shard_id: &str,
        issued_at: SystemTime,
    ) -> Self {
        Self {
            stream_arn: stream_arn.to_string(),
            stream_created_at_millis: epoch_millis(stream_created_at),
            next_shard_id: next_shard_id.to_string(),
            issued_at_millis: epoch_millis(issued_at),
        }
    }

    /// This token as the opaque `NextToken` a client is handed.
    ///
    /// Opaque (hex of a tagged byte string) because the model gives `NextToken` no
    /// structure: a client passes back what it was handed rather than constructing one. Two
    /// hex characters per byte keeps it far inside the modelled length 1..1048576.
    pub fn encode(&self) -> String {
        hex::encode(format!(
            "{SHARD_PAGE_TOKEN_TAG}:{}:{}:{}:{}",
            self.issued_at_millis,
            self.stream_created_at_millis,
            self.next_shard_id,
            self.stream_arn
        ))
    }

    /// The token `token` carries, or `None` when it is not one this service issued.
    pub fn decode(token: &str) -> Option<Self> {
        let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
        // The ARN carries colons of its own, so it is the LAST field: the tag, the two
        // instants and the shard id (whose `@pattern` admits no colon) are taken first.
        let mut fields = decoded.splitn(5, ':');
        if fields.next()? != SHARD_PAGE_TOKEN_TAG {
            return None;
        }
        let issued_at_millis = fields.next()?.parse().ok()?;
        let stream_created_at_millis = fields.next()?.parse().ok()?;
        let next_shard_id = fields.next()?.to_string();
        let stream_arn = fields.next()?.to_string();
        if next_shard_id.is_empty() || stream_arn.is_empty() {
            return None;
        }
        Some(Self {
            stream_arn,
            stream_created_at_millis,
            next_shard_id,
            issued_at_millis,
        })
    }

    /// Whether the token names `stream`: the same ARN, created at the same instant.
    pub fn names(&self, stream: &Stream) -> bool {
        self.stream_arn == stream.arn
            && self.stream_created_at_millis == epoch_millis(stream.created_at)
    }

    /// Whether the token is past its lifetime at `now` ([`LIST_SHARDS_TOKEN_TTL`]).
    ///
    /// A mint instant no clock of this emulator could have produced — one that cannot be
    /// represented, or one still in the future — names no page either, so it counts as past
    /// its lifetime rather than reaching the store.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        is_past_token_lifetime(self.issued_at_millis, now)
    }
}

/// What a `ListStreams` pagination token is tagged with, so a string the service never
/// issued is refused rather than read as a stream name.
const STREAM_PAGE_TOKEN_TAG: &str = "listStreams";

/// Where a truncated `ListStreams` page continues, and whose streams it continues.
///
/// A stream name is scoped to one region, so the token carries the region the page was
/// served for beside the stream the next page starts at; and it expires, so it carries the
/// instant it was minted. All of it travels INSIDE the token: a read mints one without
/// writing anything a later caller could see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamPageToken {
    /// The region whose streams are being listed.
    pub region: String,
    /// The name of the first stream the page did NOT return, which the next page starts at.
    pub next_stream_name: String,
    /// When the page that handed this token out was served, in milliseconds since the
    /// epoch.
    pub issued_at_millis: u128,
}

impl StreamPageToken {
    /// The token that resumes `region`'s listing at `next_stream_name`, minted at
    /// `issued_at`.
    pub fn new(region: &str, next_stream_name: &str, issued_at: SystemTime) -> Self {
        Self {
            region: region.to_string(),
            next_stream_name: next_stream_name.to_string(),
            issued_at_millis: epoch_millis(issued_at),
        }
    }

    /// This token as the opaque `NextToken` a client is handed.
    ///
    /// Opaque (hex of a tagged byte string) because the model gives `NextToken` no
    /// structure: a client passes back what it was handed rather than constructing one. Two
    /// hex characters per byte keeps it far inside the modelled length 1..1048576.
    pub fn encode(&self) -> String {
        hex::encode(format!(
            "{STREAM_PAGE_TOKEN_TAG}:{}:{}:{}",
            self.issued_at_millis, self.region, self.next_stream_name
        ))
    }

    /// The token `token` carries, or `None` when it is not one this service issued.
    pub fn decode(token: &str) -> Option<Self> {
        let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
        let mut fields = decoded.splitn(4, ':');
        if fields.next()? != STREAM_PAGE_TOKEN_TAG {
            return None;
        }
        let issued_at_millis = fields.next()?.parse().ok()?;
        let region = fields.next()?.to_string();
        let next_stream_name = fields.next()?.to_string();
        if region.is_empty() || next_stream_name.is_empty() {
            return None;
        }
        Some(Self {
            region,
            next_stream_name,
            issued_at_millis,
        })
    }

    /// Whether the token is past its lifetime at `now` ([`PAGE_TOKEN_TTL`]).
    ///
    /// A mint instant no clock of this emulator could have produced — one that cannot be
    /// represented, or one still in the future — names no page either, so it counts as past
    /// its lifetime rather than reaching the store.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        is_past_token_lifetime(self.issued_at_millis, now)
    }
}

/// Whether a pagination token minted at `issued_at_millis` is past [`PAGE_TOKEN_TTL`] at
/// `now`, which is the ONE lifetime rule every cursor of this service is judged by.
fn is_past_token_lifetime(issued_at_millis: u128, now: SystemTime) -> bool {
    let millis = u64::try_from(issued_at_millis).unwrap_or(u64::MAX);
    let Some(issued_at) = UNIX_EPOCH.checked_add(Duration::from_millis(millis)) else {
        return true;
    };
    match now.duration_since(issued_at) {
        Ok(age) => age > PAGE_TOKEN_TTL,
        // Minted after the instant it is being used at, which no page of this emulator
        // ever is.
        Err(_) => true,
    }
}

/// What a shard iterator is tagged with, so a string the service never issued is refused
/// rather than read as a position in a shard.
const SHARD_ITERATOR_TAG: &str = "shardIterator";

/// How long a shard iterator stays usable: "A shard iterator expires 5 minutes after it is
/// returned to the requester" (`GetShardIterator`).
pub const SHARD_ITERATOR_TTL: Duration = Duration::from_secs(300);

/// Where in a shard a reader starts, which is what a shard iterator names.
///
/// The five positions are the `ShardIteratorType` values the model declares
/// (`smithy_query(query="shape",shape="ShardIteratorType")`), reduced to what a reader
/// needs to resolve a position: a sequence number, an instant, or one of the two ends of
/// the shard's retained data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShardPosition {
    /// `TRIM_HORIZON` — "the last untrimmed record in the shard in the system, which is the
    /// oldest data record in the shard".
    TrimHorizon,
    /// `LATEST` — "just after the most recent record in the shard, so that you always read
    /// the most recent data in the shard", PINNED when the iterator is handed out: a shard
    /// that already held records is pinned to the newest of them
    /// ([`Self::AfterSequenceNumber`]), so this position is only ever carried by an
    /// iterator issued for a shard that held NONE — and every record such a shard holds
    /// when it is read arrived after the iterator was issued.
    Latest,
    /// `AT_SEQUENCE_NUMBER` — "the position denoted by a specific sequence number".
    AtSequenceNumber(String),
    /// `AFTER_SEQUENCE_NUMBER` — "right after the position denoted by a specific sequence
    /// number".
    AfterSequenceNumber(String),
    /// `AT_TIMESTAMP` — "the position denoted by a specific time stamp", in milliseconds
    /// since the epoch, as `GetShardIteratorInput$Timestamp` documents ("A time stamp is the
    /// Unix epoch date with precision in milliseconds").
    AtTimestamp(u128),
}

impl ShardPosition {
    /// How the position travels inside an iterator: the `ShardIteratorType` that names it
    /// and, for the three types that take one, the value the request supplied.
    fn encoded(&self) -> String {
        match self {
            Self::TrimHorizon => SHARD_ITERATOR_TYPE_TRIM_HORIZON.to_string(),
            Self::Latest => SHARD_ITERATOR_TYPE_LATEST.to_string(),
            Self::AtSequenceNumber(sequence_number) => {
                format!("{SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER}={sequence_number}")
            }
            Self::AfterSequenceNumber(sequence_number) => {
                format!("{SHARD_ITERATOR_TYPE_AFTER_SEQUENCE_NUMBER}={sequence_number}")
            }
            Self::AtTimestamp(millis) => {
                format!("{SHARD_ITERATOR_TYPE_AT_TIMESTAMP}={millis}")
            }
        }
    }

    /// The position `encoded` carries, or `None` for a field no [`Self::encoded`] produces.
    fn decoded(encoded: &str) -> Option<Self> {
        let (iterator_type, value) = match encoded.split_once('=') {
            Some((iterator_type, value)) => (iterator_type, Some(value)),
            None => (encoded, None),
        };
        match (iterator_type, value) {
            (SHARD_ITERATOR_TYPE_TRIM_HORIZON, None) => Some(Self::TrimHorizon),
            (SHARD_ITERATOR_TYPE_LATEST, None) => Some(Self::Latest),
            (SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER, Some(value)) => {
                (!value.is_empty()).then(|| Self::AtSequenceNumber(value.to_string()))
            }
            (SHARD_ITERATOR_TYPE_AFTER_SEQUENCE_NUMBER, Some(value)) => {
                (!value.is_empty()).then(|| Self::AfterSequenceNumber(value.to_string()))
            }
            (SHARD_ITERATOR_TYPE_AT_TIMESTAMP, Some(value)) => {
                value.parse().ok().map(Self::AtTimestamp)
            }
            _ => None,
        }
    }
}

/// The `ShardIteratorType` values the model declares
/// (`smithy_query(query="shape",shape="ShardIteratorType")`). Each names one
/// [`ShardPosition`], and the three that take a value are the ones whose documentation says
/// which member supplies it.
pub const SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER: &str = "AT_SEQUENCE_NUMBER";
/// See [`SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER`].
pub const SHARD_ITERATOR_TYPE_AFTER_SEQUENCE_NUMBER: &str = "AFTER_SEQUENCE_NUMBER";
/// See [`SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER`].
pub const SHARD_ITERATOR_TYPE_AT_TIMESTAMP: &str = "AT_TIMESTAMP";
/// See [`SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER`].
pub const SHARD_ITERATOR_TYPE_TRIM_HORIZON: &str = "TRIM_HORIZON";
/// See [`SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER`].
pub const SHARD_ITERATOR_TYPE_LATEST: &str = "LATEST";

/// Which shard of which stream a reader is positioned in, where in it, and when the
/// iterator was handed out.
///
/// "A shard iterator specifies the shard position from which to start reading data records
/// sequentially" (`GetShardIterator`), and the position is all a later `GetRecords` needs,
/// so every part of it travels INSIDE the iterator: handing one out writes nothing a later
/// caller could see. The stream's ARN and creation instant are both carried, so a name a
/// delete released and a later create took again is not read with the earlier stream's
/// iterator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardIteratorToken {
    /// The ARN of the stream the shard belongs to.
    pub stream_arn: String,
    /// When that stream was created, in milliseconds since the epoch.
    pub stream_created_at_millis: u128,
    /// The id of the shard being read.
    pub shard_id: String,
    /// Where in the shard the reader starts.
    pub position: ShardPosition,
    /// When the iterator was handed out, in milliseconds since the epoch.
    pub issued_at_millis: u128,
}

impl ShardIteratorToken {
    /// The iterator that starts at `position` in `shard_id` of `stream`, handed out at
    /// `issued_at`.
    pub fn new(
        stream: &Stream,
        shard_id: &str,
        position: ShardPosition,
        issued_at: SystemTime,
    ) -> Self {
        Self {
            stream_arn: stream.arn.clone(),
            stream_created_at_millis: epoch_millis(stream.created_at),
            shard_id: shard_id.to_string(),
            position,
            issued_at_millis: epoch_millis(issued_at),
        }
    }

    /// This iterator as the opaque `ShardIterator` a client is handed.
    ///
    /// Opaque (hex of a tagged byte string) because the model gives `ShardIterator` no
    /// structure: a client passes back what it was handed rather than constructing one. Two
    /// hex characters per byte keeps a stream ARN, a shard id and a position inside the
    /// modelled length 1..512.
    pub fn encode(&self) -> String {
        hex::encode(format!(
            "{SHARD_ITERATOR_TAG}:{}:{}:{}:{}:{}",
            self.issued_at_millis,
            self.stream_created_at_millis,
            self.shard_id,
            self.position.encoded(),
            self.stream_arn
        ))
    }

    /// The iterator `iterator` carries, or `None` when it is not one this service issued.
    pub fn decode(iterator: &str) -> Option<Self> {
        let decoded = String::from_utf8(hex::decode(iterator).ok()?).ok()?;
        // The ARN carries colons of its own, so it is the LAST field: the tag, the two
        // instants, the shard id (whose `@pattern` admits no colon) and the position (whose
        // own fields carry none) are taken first.
        let mut fields = decoded.splitn(6, ':');
        if fields.next()? != SHARD_ITERATOR_TAG {
            return None;
        }
        let issued_at_millis = fields.next()?.parse().ok()?;
        let stream_created_at_millis = fields.next()?.parse().ok()?;
        let shard_id = fields.next()?.to_string();
        let position = ShardPosition::decoded(fields.next()?)?;
        let stream_arn = fields.next()?.to_string();
        if shard_id.is_empty() || stream_arn.is_empty() {
            return None;
        }
        Some(Self {
            stream_arn,
            stream_created_at_millis,
            shard_id,
            position,
            issued_at_millis,
        })
    }

    /// Whether the iterator names `stream`: the same ARN, created at the same instant.
    pub fn names(&self, stream: &Stream) -> bool {
        self.stream_arn == stream.arn
            && self.stream_created_at_millis == epoch_millis(stream.created_at)
    }

    /// Whether the iterator is past its lifetime at `now` ([`SHARD_ITERATOR_TTL`]).
    ///
    /// A mint instant no clock of this emulator could have produced — one that cannot be
    /// represented, or one still in the future — names no position either, so it counts as
    /// past its lifetime rather than reaching the store.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        let millis = u64::try_from(self.issued_at_millis).unwrap_or(u64::MAX);
        let Some(issued_at) = UNIX_EPOCH.checked_add(Duration::from_millis(millis)) else {
            return true;
        };
        match now.duration_since(issued_at) {
            Ok(age) => age > SHARD_ITERATOR_TTL,
            // Handed out after the instant it is being used at, which no iterator of this
            // emulator ever is.
            Err(_) => true,
        }
    }
}

/// How a read states that the stream the request named holds no shard under the `ShardId`
/// it named. Built FROM the request, so the shard and the stream at fault name it, the way
/// [`stream_not_found_message`] names a missing stream.
pub fn shard_not_found_message(shard_id: &str, stream_identifier: &str) -> String {
    format!(
        "shard {shard_id} of stream {stream_identifier} not found under account {}",
        ::engine::emulator_account_id()
    )
}

/// How a write states that the stream it addresses has no open shard left to place a
/// record in — every shard of the map has been closed by a reshard.
pub const NO_OPEN_SHARD_MESSAGE: &str =
    "the stream has no open shard to write to; a record is written to an open shard of the \
     shard map";

/// How a read refuses a `ShardIterator` this service never issued: it names no position in
/// any shard, so it is the modelled "a specified parameter ... can't be used" refusal
/// rather than an expiry. The sentence Kinesis answers for that error class.
pub const INVALID_SHARD_ITERATOR_MESSAGE: &str = "Invalid ShardIterator.";

/// How a read refuses a request whose `StreamARN` names a different stream than its
/// `ShardIterator` does: the iterator already names the stream its position is in, so the
/// pair has to name ONE stream — the same rule [`STREAM_IDENTIFIER_MISMATCH_MESSAGE`]
/// states for a request that names a stream twice.
pub const ITERATOR_STREAM_MISMATCH_MESSAGE: &str =
    "StreamARN names a different stream than the ShardIterator does; the iterator already \
     identifies the stream its position is in";

/// How a read refuses an iterator past its lifetime: the sentence Kinesis answers for
/// `ExpiredIteratorException` ("The provided iterator exceeds the maximum age allowed"),
/// whose variable parts are the instant the iterator was handed out, the instant it was
/// used at, and the lifetime `GetShardIterator` documents ("A shard iterator expires 5
/// minutes after it is returned to the requester"), all taken from the iterator and the
/// clock.
pub fn iterator_expired_message(issued_at_millis: u128, now_millis: u128) -> String {
    format!(
        "Iterator expired. The iterator was created at time {} while right now it is {} \
         which is further in the past than the tolerated delay of {} milliseconds.",
        instant_text(issued_at_millis),
        instant_text(now_millis),
        SHARD_ITERATOR_TTL.as_millis()
    )
}

/// An instant as a refusal spells one out, in the date-time form the wire carries a
/// modelled `Timestamp` in.
fn instant_text(millis: u128) -> String {
    let millis = i64::try_from(millis).unwrap_or(i64::MAX);
    ::aws_smithy_types::DateTime::from_millis(millis)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// How a request that pairs `NextToken` with a member the token already determines is
/// refused: "Don't specify StreamName or StreamCreationTimestamp if you specify NextToken
/// because the latter unambiguously identifies the stream" (`ListShardsInput$NextToken`),
/// which holds for every listing of this service that hands out a cursor. Built FROM the
/// request, so the member at fault names it.
pub fn next_token_conflict_message(member: &str) -> String {
    format!(
        "{member} cannot be specified together with NextToken, which already identifies \
         what is being listed and where the listing resumes"
    )
}

/// How a read refuses a `NextToken` that is past its lifetime, or one this service never
/// issued: the rule `ExpiredNextTokenException` publishes ("The pagination token passed to
/// the operation is expired") together with the lifetime a cursor of this service carries
/// ([`PAGE_TOKEN_TTL`]).
pub fn next_token_expired_message() -> String {
    format!(
        "the pagination token is expired or was not issued by this service; a NextToken is \
         usable for {} seconds after the page that returned it",
        PAGE_TOKEN_TTL.as_secs()
    )
}
