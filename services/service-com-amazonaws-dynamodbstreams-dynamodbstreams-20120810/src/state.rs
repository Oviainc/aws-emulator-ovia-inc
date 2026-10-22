// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-dynamodbstreams-dynamodbstreams-20120810 service.
//!
//! DynamoDB Streams models no create or delete API of its own: a stream comes into
//! existence when DynamoDB's `CreateTable`/`UpdateTable` is given a
//! `StreamSpecification`, and records are published by `PutItem`/`UpdateItem`/
//! `DeleteItem`. All four modelled operations are therefore pure consumers, so this
//! module owns both the consumer-visible store and the publishing seam the DynamoDB
//! service calls into once it is implemented: `register_stream`, `append_record` and
//! `roll_shard` on the state struct below.
//!
//! Positions inside a shard are ABSOLUTE record ordinals (`trimmed + offset`), not
//! indices into the retained `Vec`, so trimming the oldest records past the
//! retention limit never silently moves a shard iterator that a client is still
//! holding.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{AttributeValue, KeySchemaElement};

/// A DynamoDB item image or key map, as the modelled `AttributeMap` shape.
pub type AttributeMap = HashMap<String, AttributeValue>;

/// How long a shard iterator stays usable.
///
/// From the model's `GetShardIterator` documentation: "A shard iterator expires 15
/// minutes after it is returned to the requester."
pub const SHARD_ITERATOR_TTL: Duration = Duration::from_secs(15 * 60);

/// How long a stream record is retained before it is trimmed.
///
/// From the model's `TrimmedDataAccessException` documentation: "In DynamoDB
/// Streams, there is a 24 hour limit on data retention."
pub const RECORD_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// How long after registration a stream reports `ENABLED` instead of `ENABLING`.
///
/// DynamoDB enables a stream asynchronously, which is why the model documents
/// `StreamStatus` `ENABLING` ("Streams is currently being enabled on the DynamoDB
/// table") beside `ENABLED`. The real wait is not documented, so this is a CHOSEN
/// span inside the emulator's 250–750 ms band: short enough not to slow a caller
/// down, long enough that a caller which describes the stream immediately still
/// observes the transient status.
pub const STREAM_ENABLE_DELAY: Duration = Duration::from_millis(500);

/// The most records one `GetRecords` call may return.
///
/// From the model's `GetRecords` documentation: "GetRecords can retrieve a maximum
/// of 1 MB of data or 1000 stream records, whichever comes first."
pub const MAX_GET_RECORDS_LIMIT: i32 = 1000;

/// The most data one `GetRecords` call may return, the other half of the same
/// documented sentence ("a maximum of 1 MB of data or 1000 stream records,
/// whichever comes first"). Measured over the records' own `SizeBytes`.
pub const MAX_GET_RECORDS_BYTES: i64 = 1024 * 1024;

/// The most entries one page of `ListStreams` or `DescribeStream` may return.
///
/// From the model's `ListStreamsInput$Limit` ("The maximum number of streams to
/// return. The upper limit is 100.") and `DescribeStreamInput$Limit` ("The maximum
/// number of shard objects to return. The upper limit is 100.").
pub const MAX_PAGE_LIMIT: i32 = 100;

/// `eventSource` for every DynamoDB Streams record: the model's
/// `Record$eventSource` says "For DynamoDB Streams, this is `aws:dynamodb`".
pub const EVENT_SOURCE: &str = "aws:dynamodb";

/// How deeply an item image handed to [`ComAmazonawsDynamodbstreamsDynamodbstreams20120810State::append_record`]
/// may nest.
///
/// DynamoDB's published quota: "Attribute nesting is limited to 32 levels deep."
/// (Service, Account, and Table Quotas in Amazon DynamoDB — Attributes.) An image
/// the publisher itself would have refused is refused here too, which also bounds
/// every later walk over a stored image (sizing, serialization) by a constant
/// instead of by whatever a client sent.
pub const MAX_ATTRIBUTE_NESTING_DEPTH: usize = 32;

/// The stream-record format version this emulator emits (`Record$eventVersion`).
pub const EVENT_VERSION: &str = "1.1";

/// `SequenceNumber` is a decimal string of 21..40 characters ("String contains
/// numeric characters only"), so the monotonic counter is offset into the 21-digit
/// range: every generated sequence number is exactly 21 digits.
const SEQUENCE_NUMBER_BASE: u128 = 100_000_000_000_000_000_000;

/// The format of the records a stream captures (the modelled `StreamViewType`
/// enum; wire values from `STREAM_VIEW_TYPE_VALUES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamViewType {
    KeysOnly,
    NewImage,
    OldImage,
    NewAndOldImages,
}

impl StreamViewType {
    /// The wire value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::KeysOnly => "KEYS_ONLY",
            Self::NewImage => "NEW_IMAGE",
            Self::OldImage => "OLD_IMAGE",
            Self::NewAndOldImages => "NEW_AND_OLD_IMAGES",
        }
    }

    /// Parse a wire value; `None` for anything the enum does not declare.
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "KEYS_ONLY" => Some(Self::KeysOnly),
            "NEW_IMAGE" => Some(Self::NewImage),
            "OLD_IMAGE" => Some(Self::OldImage),
            "NEW_AND_OLD_IMAGES" => Some(Self::NewAndOldImages),
            _ => None,
        }
    }

    /// Whether this view type captures the item as it looked after the change.
    #[must_use]
    pub fn captures_new_image(self) -> bool {
        matches!(self, Self::NewImage | Self::NewAndOldImages)
    }

    /// Whether this view type captures the item as it looked before the change.
    #[must_use]
    pub fn captures_old_image(self) -> bool {
        matches!(self, Self::OldImage | Self::NewAndOldImages)
    }
}

/// The kind of data modification a record describes (the modelled `OperationType`
/// enum; wire values from `OPERATION_TYPE_VALUES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationType {
    Insert,
    Modify,
    Remove,
}

impl OperationType {
    /// The wire value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "INSERT",
            Self::Modify => "MODIFY",
            Self::Remove => "REMOVE",
        }
    }

    /// Parse a wire value; `None` for anything the enum does not declare.
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "INSERT" => Some(Self::Insert),
            "MODIFY" => Some(Self::Modify),
            "REMOVE" => Some(Self::Remove),
            _ => None,
        }
    }
}

/// A stream's lifecycle status (the modelled `StreamStatus` enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStatus {
    Enabling,
    Enabled,
    Disabling,
    Disabled,
}

impl StreamStatus {
    /// The wire value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enabling => "ENABLING",
            Self::Enabled => "ENABLED",
            Self::Disabling => "DISABLING",
            Self::Disabled => "DISABLED",
        }
    }
}

/// What the publisher (DynamoDB `CreateTable`/`UpdateTable` with a
/// `StreamSpecification`) tells this service about a new stream.
#[derive(Debug, Clone)]
pub struct StreamRegistration {
    pub table_name: String,
    pub stream_view_type: StreamViewType,
    pub key_schema: Vec<KeySchemaElement>,
    pub region: String,
}

/// One item-level change the publisher is handing to the stream. Images outside
/// the stream's `StreamViewType` are dropped on append, so a consumer can never
/// see more than the stream captures.
#[derive(Debug, Clone)]
pub struct RecordAppend {
    pub event_name: OperationType,
    pub keys: AttributeMap,
    pub new_image: Option<AttributeMap>,
    pub old_image: Option<AttributeMap>,
}

/// Why [`ComAmazonawsDynamodbstreamsDynamodbstreams20120810State::append_record`]
/// refused a publication, so the publisher can word its own failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppendRejected {
    /// No stream with that ARN is registered.
    UnknownStream,
    /// Every shard of the stream is closed, so nothing can be appended.
    NoOpenShard,
    /// An image nests deeper than [`MAX_ATTRIBUTE_NESTING_DEPTH`].
    NestingTooDeep { depth: usize, limit: usize },
}

impl std::fmt::Display for AppendRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownStream => write!(f, "no stream with that ARN is registered"),
            Self::NoOpenShard => write!(f, "the stream has no open shard to append to"),
            Self::NestingTooDeep { depth, limit } => write!(
                f,
                "an item image nests {depth} levels deep, past the {limit} level limit"
            ),
        }
    }
}

/// A stream record as it is persisted.
#[derive(Debug, Clone)]
pub struct StoredRecord {
    pub event_id: String,
    pub event_name: OperationType,
    pub event_version: String,
    pub event_source: String,
    pub aws_region: String,
    pub sequence_number: String,
    pub approximate_creation_date_time: SystemTime,
    pub keys: AttributeMap,
    pub new_image: Option<AttributeMap>,
    pub old_image: Option<AttributeMap>,
    pub size_bytes: i64,
}

/// A shard: an append-ordered run of records, open until it is rolled over.
#[derive(Debug, Clone)]
pub struct StoredShard {
    pub shard_id: String,
    pub parent_shard_id: Option<String>,
    pub starting_sequence_number: String,
    /// `None` while the shard is open (still writable), which is why
    /// `SequenceNumberRange.EndingSequenceNumber` is absent for an open shard.
    pub ending_sequence_number: Option<String>,
    /// The records still retained, oldest first.
    pub records: VecDeque<StoredRecord>,
    /// How many records this shard has already trimmed past the retention limit.
    pub trimmed: u64,
}

impl StoredShard {
    /// Whether the shard is still writable.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.ending_sequence_number.is_none()
    }

    /// Total records ever appended, trimmed ones included: the absolute position
    /// just past the newest record, which is where `LATEST` starts.
    #[must_use]
    pub fn appended(&self) -> u64 {
        self.trimmed + self.records.len() as u64
    }

    /// The absolute position of the oldest retained record, which is where
    /// `TRIM_HORIZON` starts.
    #[must_use]
    pub fn trim_horizon(&self) -> u64 {
        self.trimmed
    }

    /// Up to `limit` records starting at absolute position `position`. Empty when
    /// the position is already past the newest record.
    #[must_use]
    pub fn records_from(&self, position: u64, limit: usize) -> Vec<&StoredRecord> {
        let Some(offset) = position.checked_sub(self.trimmed) else {
            return Vec::new();
        };
        let Ok(offset) = usize::try_from(offset) else {
            return Vec::new();
        };
        self.records.iter().skip(offset).take(limit).collect()
    }
}

/// A stream and the shards that comprise it.
#[derive(Debug, Clone)]
pub struct StoredStream {
    pub stream_arn: String,
    pub table_name: String,
    pub stream_label: String,
    pub stream_view_type: StreamViewType,
    pub creation_request_date_time: SystemTime,
    /// When the asynchronous enable this registration started becomes due.
    pub enabled_at: SystemTime,
    pub aws_region: String,
    pub key_schema: Vec<KeySchemaElement>,
    /// Shards in creation order: a child shard always follows its parent.
    pub shards: Vec<StoredShard>,
}

impl StoredStream {
    /// The status the clock owes at `now`: a registration starts the stream
    /// `ENABLING` and it becomes `ENABLED` once the enable is due.
    #[must_use]
    pub fn status_at(&self, now: SystemTime) -> StreamStatus {
        if now < self.enabled_at {
            StreamStatus::Enabling
        } else {
            StreamStatus::Enabled
        }
    }

    /// The shard with this id, if the stream has one.
    #[must_use]
    pub fn shard(&self, shard_id: &str) -> Option<&StoredShard> {
        self.shards.iter().find(|shard| shard.shard_id == shard_id)
    }
}

/// Where a shard iterator token points, and when it was handed out.
#[derive(Debug, Clone)]
pub struct IteratorState {
    pub stream_arn: String,
    pub shard_id: String,
    /// The ABSOLUTE position of the next record to deliver.
    pub next_position: u64,
    pub issued_at: SystemTime,
}

/// Per-service state for com-amazonaws-dynamodbstreams-dynamodbstreams-20120810. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDynamodbstreamsDynamodbstreams20120810State {
    /// Registered streams, keyed and ordered by stream ARN so `ListStreams` pages
    /// deterministically.
    streams: BTreeMap<String, StoredStream>,
    /// Live shard iterators, keyed by the opaque token handed to the client.
    iterators: HashMap<String, IteratorState>,
    /// Backs sequence-number generation; monotonic for the life of the emulator.
    next_sequence_number: u64,
    /// Backs shard-id generation, so two shards created in the same millisecond
    /// still get distinct ids.
    next_shard_ordinal: u64,
    /// Backs iterator-token generation, so two iterators on the same position are
    /// still distinct tokens.
    next_iterator_ordinal: u64,
}

impl ComAmazonawsDynamodbstreamsDynamodbstreams20120810State {
    /// Every registered stream, in stream-ARN order.
    #[must_use]
    pub fn streams(&self) -> &BTreeMap<String, StoredStream> {
        &self.streams
    }

    /// The stream with this ARN, if one is registered.
    #[must_use]
    pub fn stream(&self, stream_arn: &str) -> Option<&StoredStream> {
        self.streams.get(stream_arn)
    }

    /// Where a shard-iterator token points, if it is still live.
    #[must_use]
    pub fn iterator(&self, token: &str) -> Option<&IteratorState> {
        self.iterators.get(token)
    }

    /// Materialise everything the CLOCK already owes at `now`, whether or not
    /// anybody called: records past the 24 hour retention limit are trimmed, and
    /// shard iterators past their 15 minute lifetime are retired.
    pub fn materialize_due_expiries(&mut self, now: SystemTime) {
        for stream in self.streams.values_mut() {
            for shard in &mut stream.shards {
                while shard
                    .records
                    .front()
                    .is_some_and(|record| older_than(record.approximate_creation_date_time, now, RECORD_RETENTION))
                {
                    shard.records.pop_front();
                    shard.trimmed += 1;
                }
            }
        }
        self.iterators
            .retain(|_, iterator| !older_than(iterator.issued_at, now, SHARD_ITERATOR_TTL));
    }

    /// Register a stream for a table — the publishing seam DynamoDB's
    /// `CreateTable`/`UpdateTable` calls when a `StreamSpecification` enables one.
    ///
    /// Returns the stream ARN. Idempotent: registering the same table at the same
    /// instant yields the same ARN and leaves the existing stream untouched, so a
    /// repeated `UpdateTable` does not fork a second stream.
    pub fn register_stream(
        &mut self,
        registration: &StreamRegistration,
        at: SystemTime,
    ) -> String {
        let stream_label = iso8601_label(at);
        let stream_arn = ::engine::emulator_arn(
            "dynamodb",
            &registration.region,
            &format!(
                "table/{}/stream/{}",
                registration.table_name, stream_label
            ),
        );
        if self.streams.contains_key(&stream_arn) {
            return stream_arn;
        }
        let shard = self.new_shard(&stream_arn, None, at);
        self.streams.insert(
            stream_arn.clone(),
            StoredStream {
                stream_arn: stream_arn.clone(),
                table_name: registration.table_name.clone(),
                stream_label,
                stream_view_type: registration.stream_view_type,
                creation_request_date_time: at,
                enabled_at: at + STREAM_ENABLE_DELAY,
                aws_region: registration.region.clone(),
                key_schema: registration.key_schema.clone(),
                shards: vec![shard],
            },
        );
        stream_arn
    }

    /// Publish one item-level change onto the stream's open shard — the seam
    /// DynamoDB's `PutItem`/`UpdateItem`/`DeleteItem` call.
    ///
    /// Returns the sequence number assigned to the record, or the reason it was
    /// refused: no such stream, no open shard, or an image nesting past
    /// [`MAX_ATTRIBUTE_NESTING_DEPTH`]. Refusing the over-deep image HERE is what
    /// keeps every later walk over a stored image bounded.
    pub fn append_record(
        &mut self,
        stream_arn: &str,
        event: &RecordAppend,
        at: SystemTime,
    ) -> Result<String, AppendRejected> {
        for image in [
            Some(&event.keys),
            event.new_image.as_ref(),
            event.old_image.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            let depth = attribute_map_depth(image);
            if depth > MAX_ATTRIBUTE_NESTING_DEPTH {
                return Err(AppendRejected::NestingTooDeep {
                    depth,
                    limit: MAX_ATTRIBUTE_NESTING_DEPTH,
                });
            }
        }

        let sequence_number = format_sequence_number(self.next_sequence_number);
        let stream = self
            .streams
            .get_mut(stream_arn)
            .ok_or(AppendRejected::UnknownStream)?;
        let view_type = stream.stream_view_type;
        let aws_region = stream.aws_region.clone();
        let shard = stream
            .shards
            .iter_mut()
            .rfind(|shard| shard.is_open())
            .ok_or(AppendRejected::NoOpenShard)?;

        let new_image = view_type
            .captures_new_image()
            .then(|| event.new_image.clone())
            .flatten();
        let old_image = view_type
            .captures_old_image()
            .then(|| event.old_image.clone())
            .flatten();
        let size_bytes = record_size_bytes(&event.keys, new_image.as_ref(), old_image.as_ref());

        shard.records.push_back(StoredRecord {
            event_id: event_id(stream_arn, &shard.shard_id, &sequence_number),
            event_name: event.event_name,
            event_version: EVENT_VERSION.to_string(),
            event_source: EVENT_SOURCE.to_string(),
            aws_region,
            sequence_number: sequence_number.clone(),
            approximate_creation_date_time: at,
            keys: event.keys.clone(),
            new_image,
            old_image,
            size_bytes,
        });
        self.next_sequence_number += 1;
        Ok(sequence_number)
    }

    /// Close the stream's open shard and open a child shard on it, the way
    /// DynamoDB rolls a shard over as a stream ages.
    ///
    /// Returns the new shard's id, or `None` when the stream is not registered or
    /// has no open shard.
    pub fn roll_shard(&mut self, stream_arn: &str, at: SystemTime) -> Option<String> {
        let (closing_shard_id, ending_sequence_number) = {
            let stream = self.streams.get(stream_arn)?;
            let open = stream.shards.iter().rfind(|shard| shard.is_open())?;
            // A shard that never took a record still needs an ending sequence
            // number of 21..40 digits, so it closes on its own starting number.
            let ending = open
                .records
                .back()
                .map_or_else(|| open.starting_sequence_number.clone(), |record| {
                    record.sequence_number.clone()
                });
            (open.shard_id.clone(), ending)
        };
        let child = self.new_shard(stream_arn, Some(closing_shard_id.clone()), at);
        let child_shard_id = child.shard_id.clone();
        let stream = self.streams.get_mut(stream_arn)?;
        if let Some(closing) = stream
            .shards
            .iter_mut()
            .find(|shard| shard.shard_id == closing_shard_id)
        {
            closing.ending_sequence_number = Some(ending_sequence_number);
        }
        stream.shards.push(child);
        Some(child_shard_id)
    }

    /// Record a shard iterator against the position it names and return the
    /// opaque token the client gets. The token carries no position itself: it is
    /// resolved here, so a client cannot forge or decode one.
    pub fn mint_iterator(
        &mut self,
        stream_arn: &str,
        shard_id: &str,
        next_position: u64,
        at: SystemTime,
    ) -> String {
        let ordinal = self.next_iterator_ordinal;
        self.next_iterator_ordinal += 1;
        let token = opaque_token(stream_arn, shard_id, next_position, ordinal);
        self.iterators.insert(
            token.clone(),
            IteratorState {
                stream_arn: stream_arn.to_string(),
                shard_id: shard_id.to_string(),
                next_position,
                issued_at: at,
            },
        );
        token
    }

    fn new_shard(
        &mut self,
        stream_arn: &str,
        parent_shard_id: Option<String>,
        at: SystemTime,
    ) -> StoredShard {
        let ordinal = self.next_shard_ordinal;
        self.next_shard_ordinal += 1;
        StoredShard {
            shard_id: shard_id(stream_arn, ordinal, at),
            parent_shard_id,
            starting_sequence_number: format_sequence_number(self.next_sequence_number),
            ending_sequence_number: None,
            records: VecDeque::new(),
            trimmed: 0,
        }
    }
}

/// Whether `at` is more than `ttl` before `now`.
fn older_than(at: SystemTime, now: SystemTime, ttl: Duration) -> bool {
    now.duration_since(at).is_ok_and(|age| age > ttl)
}

/// Milliseconds since the Unix epoch, clamped at the epoch for an earlier instant.
#[must_use]
pub fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_millis()).ok())
        .unwrap_or_default()
}

/// A stream label: "A timestamp, in ISO 8601 format, for this stream"
/// (`Stream$StreamLabel`), e.g. `2026-10-21T21:58:07.123`.
#[must_use]
pub fn iso8601_label(at: SystemTime) -> String {
    let millis = epoch_millis(at);
    let rendered = ::aws_smithy_types::DateTime::from_millis(millis)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    // `Format::DateTime` renders RFC 3339 with a `Z` zone; the label AWS emits for
    // a stream is the same instant without it.
    let trimmed = rendered.strip_suffix('Z').unwrap_or(&rendered);
    if trimmed.is_empty() {
        // Unreachable for any instant `from_millis` accepts, but a label must
        // never be empty: the stream ARN is built from it.
        return millis.to_string();
    }
    trimmed.to_string()
}

/// A sequence number for the `n`th record: a 21-digit decimal string, so the
/// fixed width makes the modelled "numeric characters only" ordering total.
#[must_use]
pub fn format_sequence_number(n: u64) -> String {
    (SEQUENCE_NUMBER_BASE + u128::from(n)).to_string()
}

/// Compare two decimal sequence-number strings NUMERICALLY.
///
/// `SequenceNumber` is 21..40 characters of digits, which overflows every fixed
/// integer width, so the comparison is by significant-digit count and then
/// lexicographic.
#[must_use]
pub fn compare_sequence_numbers(left: &str, right: &str) -> std::cmp::Ordering {
    let left = left.trim_start_matches('0');
    let right = right.trim_start_matches('0');
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

/// Whether a client-supplied sequence number is well formed: the model documents
/// `SequenceNumber` as containing "numeric characters only", over the declared
/// length bound of 21..40 characters.
#[must_use]
pub fn is_sequence_number(value: &str) -> bool {
    (21..=40).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// A shard id of the shape AWS emits — `shardId-<20 digits>-<8 hex>`, 37
/// characters, inside the modelled 28..65 bound. The digits are the shard's
/// creation instant and the suffix is derived from the stream ARN and the shard's
/// ordinal, so the id is unique and reproducible rather than random.
fn shard_id(stream_arn: &str, ordinal: u64, at: SystemTime) -> String {
    let digest = digest_hex(&[stream_arn.as_bytes(), &ordinal.to_be_bytes()]);
    format!(
        "shardId-{:020}-{}",
        epoch_millis(at).unsigned_abs(),
        &digest[..8]
    )
}

/// A `Record.eventID`: "A globally unique identifier for the event that was
/// recorded in this stream record." Derived from the record's own coordinates, so
/// it is unique per record and stable across reads.
fn event_id(stream_arn: &str, shard_id: &str, sequence_number: &str) -> String {
    digest_hex(&[
        stream_arn.as_bytes(),
        shard_id.as_bytes(),
        sequence_number.as_bytes(),
    ])[..32]
        .to_string()
}

/// An opaque shard-iterator token, 64 hex characters (inside the modelled
/// `ShardIterator` 1..2048 bound). It is a digest, not an encoding: the position
/// it names lives only in the iterator table, so the token cannot be decoded or
/// forged into a different position.
fn opaque_token(stream_arn: &str, shard_id: &str, next_position: u64, ordinal: u64) -> String {
    digest_hex(&[
        stream_arn.as_bytes(),
        shard_id.as_bytes(),
        &next_position.to_be_bytes(),
        &ordinal.to_be_bytes(),
    ])
}

fn digest_hex(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        // Length-prefixed, so concatenating two different field splits cannot
        // produce the same digest.
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hex::encode(hasher.finalize())
}

/// `StreamRecord.SizeBytes`: "The size of the stream record, in bytes."
///
/// Sized the way DynamoDB documents an item's size — the sum of the lengths of
/// its attribute names and values — over exactly the images the stream captures.
/// The modelled member has `range min 1`, so an empty record still sizes as 1.
fn record_size_bytes(
    keys: &AttributeMap,
    new_image: Option<&AttributeMap>,
    old_image: Option<&AttributeMap>,
) -> i64 {
    let total = attribute_map_size(keys)
        .saturating_add(new_image.map_or(0, attribute_map_size))
        .saturating_add(old_image.map_or(0, attribute_map_size));
    total.max(1)
}

/// The summed length of an image's attribute names and values.
///
/// ITERATIVE, over an explicit worklist: `AttributeValue::M`/`L` nest, and a
/// recursive walk over an attacker-chosen nesting depth overflows the thread
/// stack — which in Rust aborts the process instead of unwinding. The worklist
/// grows on the heap with the image's own size, and
/// [`MAX_ATTRIBUTE_NESTING_DEPTH`] already bounds what can be stored.
fn attribute_map_size(map: &AttributeMap) -> i64 {
    let mut total: i64 = 0;
    let mut worklist: Vec<&AttributeValue> = Vec::with_capacity(map.len());
    for (name, value) in map {
        total = total.saturating_add(name.len() as i64);
        worklist.push(value);
    }
    while let Some(value) = worklist.pop() {
        let size = match value {
            AttributeValue::S(s) | AttributeValue::N(s) => s.len() as i64,
            AttributeValue::B(b) => b.len() as i64,
            AttributeValue::Ss(items) | AttributeValue::Ns(items) => {
                items.iter().map(|item| item.len() as i64).sum()
            }
            AttributeValue::Bs(items) => items.iter().map(|item| item.len() as i64).sum(),
            AttributeValue::M(inner) => {
                let mut names: i64 = 0;
                for (name, nested) in inner {
                    names = names.saturating_add(name.len() as i64);
                    worklist.push(nested);
                }
                names
            }
            AttributeValue::L(items) => {
                worklist.extend(items.iter());
                0
            }
            // A null or boolean attribute is one byte of value.
            AttributeValue::Null(_) | AttributeValue::Bool(_) => 1,
        };
        total = total.saturating_add(size);
    }
    total
}

/// How deeply an image nests: the image itself is level 1, the members of a map
/// or list attribute inside it are level 2, and so on. `0` for an empty image.
///
/// ITERATIVE for the same reason [`attribute_map_size`] is: this is the check that
/// refuses an over-deep image, so it must never be the thing that overflows.
#[must_use]
pub fn attribute_map_depth(map: &AttributeMap) -> usize {
    let mut worklist: Vec<(&AttributeValue, usize)> =
        map.values().map(|value| (value, 1usize)).collect();
    let mut deepest = if worklist.is_empty() { 0 } else { 1 };
    while let Some((value, depth)) = worklist.pop() {
        deepest = deepest.max(depth);
        match value {
            AttributeValue::M(inner) => {
                worklist.extend(inner.values().map(|nested| (nested, depth + 1)));
            }
            AttributeValue::L(items) => {
                worklist.extend(items.iter().map(|nested| (nested, depth + 1)));
            }
            _ => {}
        }
    }
    deepest
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDynamodbstreamsDynamodbstreams20120810State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDynamodbstreamsDynamodbstreams20120810State>)
}
