// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kinesis-kinesis-20131202 service.
//!
//! This module owns the stream records the implemented operations read and write: the
//! shard map `CreateStream` lays down, the ARN a stream is addressed by, and the single
//! place where the `CREATING` -> `ACTIVE` transition the clock owes a new stream is
//! decided ([`Stream::status`]).

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    EnhancedMetrics, HashKeyRange, SequenceNumberRange, Shard, StreamDescription,
    StreamDescriptionSummary, StreamModeDetails, WarmThroughputObject,
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

/// How many shards `DescribeStream` returns when the request names no `Limit`: "The
/// maximum number of shards to return in a single call. The default value is 100"
/// (`DescribeStreamInput$Limit`).
pub const DEFAULT_DESCRIBE_STREAM_SHARD_LIMIT: i32 = 100;

/// The most shards `DescribeStream` returns in one call: "If you specify a value greater
/// than 100, at most 100 results are returned" (`DescribeStreamInput$Limit`).
pub const MAX_DESCRIBE_STREAM_SHARD_LIMIT: i32 = 100;

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

/// One shard of a stream's shard map: the slice of the hash key space it covers and the
/// sequence numbers it spans.
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
}

impl ShardRecord {
    /// This record as the modelled `Shard` shape.
    pub fn to_shape(&self) -> Shard {
        Shard {
            shard_id: self.shard_id.clone(),
            // Both are set only on a shard produced by a split or a merge, which is
            // `SplitShard`/`MergeShards` work, not a create's.
            parent_shard_id: None,
            adjacent_parent_shard_id: None,
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
        self.ending_sequence_number.is_none()
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
    /// The tags the create carried.
    pub tags: HashMap<String, String>,
}

impl Stream {
    /// The stream's status at `now`: `CREATING` until the transition the create armed
    /// falls due, `ACTIVE` from then on. Derived from the stored instant, so two reads at
    /// the same instant agree and no read has to write.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.active_at {
            STREAM_CREATING
        } else {
            STREAM_ACTIVE
        }
    }

    /// How many of the stream's shards are open.
    pub fn open_shard_count(&self) -> i32 {
        i32::try_from(self.shards.iter().filter(|shard| shard.is_open()).count())
            .unwrap_or(i32::MAX)
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
    /// The stream `region`'s endpoint holds under `name`, if any.
    pub fn stream_in(&self, region: &str, name: &str) -> Option<&Stream> {
        self.streams.get(&RegionalName::new(region, name))
    }

    /// The stream `region`'s endpoint holds under `arn`, if any.
    pub fn stream_by_arn(&self, region: &str, arn: &str) -> Option<&Stream> {
        self.streams
            .values()
            .find(|stream| stream.region == region && stream.arn == arn)
    }

    /// The stream a request's `StreamName` / `StreamARN` pair names.
    ///
    /// "When invoking this API, you must use either the StreamARN or the StreamName
    /// parameter, or both" (`DescribeStream`), and that holds for every operation of this
    /// service that addresses a stream: a request naming neither is refused rather than
    /// answered, and a request naming both has to name ONE stream with them.
    pub fn resolve_stream(
        &self,
        region: &str,
        name: Option<&str>,
        arn: Option<&str>,
    ) -> Result<&Stream, StreamLookupFailure> {
        match (name, arn) {
            (None, None) => Err(StreamLookupFailure::Unspecified),
            (Some(name), None) => self
                .stream_in(region, name)
                .ok_or(StreamLookupFailure::NotFound),
            (None, Some(arn)) => self
                .stream_by_arn(region, arn)
                .ok_or(StreamLookupFailure::NotFound),
            (Some(name), Some(arn)) => {
                let stream = self
                    .stream_by_arn(region, arn)
                    .ok_or(StreamLookupFailure::NotFound)?;
                if stream.name == name {
                    Ok(stream)
                } else {
                    Err(StreamLookupFailure::Mismatch)
                }
            }
        }
    }

    /// How many streams `region`'s endpoint holds, which is what
    /// [`MAX_STREAMS_PER_REGION`] bounds.
    pub fn streams_held_in(&self, region: &str) -> usize {
        self.streams
            .keys()
            .filter(|key| key.region == region)
            .count()
    }

    /// How many shards `region`'s endpoint holds across every stream, which is what
    /// [`MAX_SHARDS_PER_REGION`] bounds.
    pub fn shards_held_in(&self, region: &str) -> u32 {
        self.streams
            .values()
            .filter(|stream| stream.region == region)
            .fold(0u32, |held, stream| {
                held.saturating_add(u32::try_from(stream.shards.len()).unwrap_or(u32::MAX))
            })
    }

    /// How many shards this emulator holds across every region, which is what
    /// [`MAX_SHARDS_TOTAL`] bounds: the ceiling no request can partition.
    pub fn shards_held(&self) -> u32 {
        self.streams.values().fold(0u32, |held, stream| {
            held.saturating_add(u32::try_from(stream.shards.len()).unwrap_or(u32::MAX))
        })
    }
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
    let millis = created_at
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or(0);
    format!("49{millis:013}{index:06}")
}
