// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-firehose-firehose-20150804 service.
//!
//! One name-keyed register of Firehose streams, plus the service-level counter that
//! mints record ids. Everything an operation answers is computed from here.
//!
//! ## Clock-owed transitions
//!
//! `CreateDeliveryStream` and `DeleteDeliveryStream` are asynchronous: the model's own
//! documentation says a created stream starts in `CREATING` and reaches `ACTIVE` later,
//! and that a deleted stream goes to `DELETING` and is removed later. Neither span is
//! documented with a value, so this emulator picks one ([`TRANSITION_DELAY`]) and stores
//! the instant the change comes due. No operation sleeps: every handler calls
//! [`ComAmazonawsFirehoseFirehose20150804State::materialise_due`] first, which applies
//! only the changes the clock has ALREADY made due — a reporting operation never moves a
//! status that was not owed at the same instant anyway.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AmazonOpenSearchServerlessDestinationConfiguration,
    AmazonopensearchserviceDestinationConfiguration, DeliveryStreamEncryptionConfiguration,
    ElasticsearchDestinationConfiguration, ExtendedS3DestinationConfiguration,
    HttpEndpointDestinationConfiguration, IcebergDestinationConfiguration,
    RedshiftDestinationConfiguration, S3DestinationConfiguration,
    SnowflakeDestinationConfiguration, SourceDescription, SplunkDestinationConfiguration,
};

/// How long after the starting call a clock-owed stream transition
/// (`CREATING` -> `ACTIVE`, `DELETING` -> removed) comes due.
///
/// AWS documents neither span, so this is a CHOSEN value, kept short enough for a test
/// to wait for it and long enough that a caller acting immediately after the starting
/// call still observes the transient status the real service shows.
pub const TRANSITION_DELAY: Duration = Duration::from_millis(600);

/// How long an ingested record stays in a stream's buffer.
///
/// Documented, not chosen: "Data records sent to Firehose are stored for 24 hours from
/// the time they are added to a Firehose stream as it tries to send the records to the
/// destination" (`PutRecord`).
pub const RECORD_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// The default per-Region stream quota: "By default, you can create up to 5,000 Firehose
/// streams per Amazon Web Services Region" (`CreateDeliveryStream`).
pub const MAX_STREAMS_PER_REGION: usize = 5_000;

/// The most record data ONE stream keeps buffered.
///
/// An emulator resource budget, NOT an AWS limit: this process holds every buffered
/// record in its own heap, so without a cap a client could put records faster than the
/// 24-hour retention reclaims them and exhaust the process. A put that does not fit is
/// refused with the modelled `ServiceUnavailableException` ("The service is unavailable.
/// Back off and retry the operation. If you continue to see the exception, throughput
/// limits for the Firehose stream may have been exceeded"), and in a batch with that
/// per-record `ErrorCode` — the answer the real service gives when a stream cannot take
/// more. Each record is at most the modelled 1,024,000 bytes, so this bounds the count
/// too; `MAX_BUFFERED_RECORDS_PER_STREAM` bounds it for small records.
pub const MAX_BUFFERED_BYTES_PER_STREAM: usize = 16 * 1024 * 1024;

/// The most record data this service keeps buffered across EVERY stream — the same
/// budget as [`MAX_BUFFERED_BYTES_PER_STREAM`], applied so many streams cannot together
/// exhaust the process.
pub const MAX_BUFFERED_BYTES: usize = 64 * 1024 * 1024;

/// The most records ONE stream keeps buffered, so a flood of tiny records cannot grow
/// the buffer (and the expiry scan over it) without bound either.
pub const MAX_BUFFERED_RECORDS_PER_STREAM: usize = 50_000;

/// The lifecycle states this emulator puts a stream through, as the wire enum
/// `DeliveryStreamStatus` names them.
///
/// `CREATING_FAILED` / `DELETING_FAILED` are modelled but unreachable here: nothing in
/// the emulator makes a create or a delete fail asynchronously.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStatus {
    Creating,
    Active,
    Deleting,
}

impl StreamStatus {
    /// The `DeliveryStreamStatus` value this state renders as on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
        }
    }
}

/// A change the clock owes a stream, applied by [`DeliveryStream::materialise_due`] once
/// its due instant has passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingTransition {
    /// `CREATING` -> `ACTIVE` (and `ENABLING` -> `ENABLED` for encryption enabled at create).
    Activate,
    /// `DELETING` -> gone from the register.
    Remove,
}

/// The destination configuration a `CreateDeliveryStream` supplied, kept in the variant
/// the caller chose so `DescribeDeliveryStream` can render the matching
/// `*DestinationDescription`.
///
/// Boxed per variant: these configurations are large structures and an unboxed enum
/// would be as big as the largest of them.
#[derive(Debug, Clone)]
pub enum DestinationConfigKind {
    S3(Box<S3DestinationConfiguration>),
    ExtendedS3(Box<ExtendedS3DestinationConfiguration>),
    Redshift(Box<RedshiftDestinationConfiguration>),
    Elasticsearch(Box<ElasticsearchDestinationConfiguration>),
    AmazonOpenSearchService(Box<AmazonopensearchserviceDestinationConfiguration>),
    Splunk(Box<SplunkDestinationConfiguration>),
    HttpEndpoint(Box<HttpEndpointDestinationConfiguration>),
    Snowflake(Box<SnowflakeDestinationConfiguration>),
    AmazonOpenSearchServerless(Box<AmazonOpenSearchServerlessDestinationConfiguration>),
    Iceberg(Box<IcebergDestinationConfiguration>),
}

/// One destination of a stream: the id Firehose assigned it and the configuration it was
/// created with.
#[derive(Debug, Clone)]
pub struct Destination {
    /// `DestinationId`, as Firehose formats it: `destinationId-` plus a zero-padded ordinal.
    pub destination_id: String,
    pub config: DestinationConfigKind,
}

/// A record `PutRecord` / `PutRecordBatch` appended to a stream's buffer.
#[derive(Debug, Clone)]
pub struct StoredRecord {
    /// The `RecordId` the put answered with — unique across this emulator's streams.
    pub record_id: String,
    pub data: Vec<u8>,
    pub received_at: SystemTime,
}

/// One Firehose stream.
#[derive(Debug, Clone)]
pub struct DeliveryStream {
    pub name: String,
    pub arn: String,
    pub status: StreamStatus,
    /// The change the clock owes this stream, with the instant it comes due.
    pub pending: Option<(PendingTransition, SystemTime)>,
    /// A `DeliveryStreamType` value.
    pub stream_type: String,
    /// Rendered as the decimal `VersionId` / `CurrentDeliveryStreamVersionId`.
    pub version_id: u64,
    pub create_timestamp: SystemTime,
    pub last_update_timestamp: SystemTime,
    pub source: Option<SourceDescription>,
    pub destinations: Vec<Destination>,
    pub encryption: Option<DeliveryStreamEncryptionConfiguration>,
    pub tags: BTreeMap<String, String>,
    pub records: Vec<StoredRecord>,
    /// The bytes [`Self::records`] holds, kept as a running total so a put checks the
    /// buffer budget without walking the buffer.
    pub buffered_bytes: usize,
    /// The ordinal the next destination of this stream gets.
    pub next_destination_id: u64,
}

impl DeliveryStream {
    /// The id for this stream's next destination, in Firehose's own format.
    pub fn mint_destination_id(&mut self) -> String {
        let ordinal = self.next_destination_id;
        self.next_destination_id += 1;
        format!("destinationId-{ordinal:012}")
    }

    /// Whether server-side encryption is in force for this stream right now.
    fn is_encrypted(&self) -> bool {
        self.encryption
            .as_ref()
            .is_some_and(|encryption| encryption.status.as_deref() == Some("ENABLED"))
    }

    /// Apply every change the clock has already made due to this stream, and report
    /// whether the stream still exists afterwards (`false` once its removal is due).
    /// Adds the bytes any expired record freed to `freed`.
    ///
    /// Writes nothing that was not already owed at `now`, so calling it from a reporting
    /// operation cannot change what a later caller sees.
    fn materialise_due(&mut self, now: SystemTime, freed: &mut usize) -> bool {
        // Records are appended in arrival order, so the expired ones are a prefix: the
        // scan costs what it drops, not what the buffer holds.
        let expired = self.records.partition_point(|record| {
            matches!(now.duration_since(record.received_at), Ok(age) if age >= RECORD_RETENTION)
        });
        if expired > 0 {
            let dropped: usize = self
                .records
                .drain(..expired)
                .map(|record| record.data.len())
                .sum();
            self.buffered_bytes = self.buffered_bytes.saturating_sub(dropped);
            *freed += dropped;
        }

        match self.pending {
            Some((PendingTransition::Activate, due)) if now >= due => {
                self.status = StreamStatus::Active;
                self.pending = None;
                if let Some(encryption) = self.encryption.as_mut() {
                    if encryption.status.as_deref() == Some("ENABLING") {
                        encryption.status = Some("ENABLED".to_string());
                    }
                }
                true
            }
            Some((PendingTransition::Remove, due)) if now >= due => false,
            _ => true,
        }
    }
}

/// Per-service state for com-amazonaws-firehose-firehose-20150804. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsFirehoseFirehose20150804State {
    /// Every stream, keyed by name. A `BTreeMap` so `ListDeliveryStreams` ordering and
    /// `ExclusiveStartDeliveryStreamName` paging are deterministic and alphabetical.
    pub streams: BTreeMap<String, DeliveryStream>,
    /// Monotonic, service-wide, so every minted `RecordId` is distinct.
    pub record_counter: u64,
    /// The bytes every stream's buffer holds together, kept as a running total so a put
    /// checks the service-wide budget without walking the register.
    pub buffered_bytes: usize,
}

impl ComAmazonawsFirehoseFirehose20150804State {
    /// Apply every clock-owed change across the register: activate streams whose
    /// creation span has elapsed, drop streams whose deletion has, and expire records
    /// past their documented retention. A removed stream takes its buffered data with it.
    pub fn materialise_due(&mut self, now: SystemTime) {
        let mut freed = 0;
        self.streams.retain(|_, stream| {
            let keep = stream.materialise_due(now, &mut freed);
            if !keep {
                freed += stream.buffered_bytes;
            }
            keep
        });
        self.buffered_bytes = self.buffered_bytes.saturating_sub(freed);
    }

    /// Append one record to `name`'s buffer: answers the `RecordId` it was stored under
    /// (or `None` when the buffer budget left no room for it, in which case nothing was
    /// stored) and whether server-side encryption was in force.
    pub fn ingest_record(
        &mut self,
        name: &str,
        data: Vec<u8>,
        now: SystemTime,
    ) -> Result<(Option<String>, bool), StreamRefusal> {
        let (stream, counter, buffered_bytes) = self.stream_accepting_records(name)?;
        let encrypted = stream.is_encrypted();
        let record_id = push_record(stream, counter, buffered_bytes, data, now);
        Ok((record_id, encrypted))
    }

    /// Append a batch to `name`'s buffer: answers one entry per record, in the order the
    /// records were given — the `RecordId` it was stored under, or `None` for a record the
    /// buffer had no room for — and whether server-side encryption was in force.
    pub fn ingest_records(
        &mut self,
        name: &str,
        records: Vec<Vec<u8>>,
        now: SystemTime,
    ) -> Result<(Vec<Option<String>>, bool), StreamRefusal> {
        let (stream, counter, buffered_bytes) = self.stream_accepting_records(name)?;
        let encrypted = stream.is_encrypted();
        let record_ids = records
            .into_iter()
            .map(|data| push_record(stream, counter, buffered_bytes, data, now))
            .collect();
        Ok((record_ids, encrypted))
    }

    /// The stream `name` names, if it exists and is accepting data.
    ///
    /// Hands back the record counter and the service-wide buffered-byte total beside it
    /// (disjoint field borrows) so a put resolves the stream ONCE and mints an id only for
    /// a record it goes on to store.
    fn stream_accepting_records(
        &mut self,
        name: &str,
    ) -> Result<(&mut DeliveryStream, &mut u64, &mut usize), StreamRefusal> {
        let counter = &mut self.record_counter;
        let buffered_bytes = &mut self.buffered_bytes;
        let Some(stream) = self.streams.get_mut(name) else {
            return Err(StreamRefusal::NoSuchStream);
        };
        // "Attempts to send data to a delivery stream that is not in the ACTIVE state
        // cause an exception."
        if stream.status != StreamStatus::Active {
            return Err(StreamRefusal::NotActive(stream.status.as_wire()));
        }
        Ok((stream, counter, buffered_bytes))
    }
}

/// Why a stream could take no data at all — the whole request is refused with a modelled
/// error.
///
/// A buffer budget ([`MAX_BUFFERED_BYTES_PER_STREAM`], [`MAX_BUFFERED_BYTES`],
/// [`MAX_BUFFERED_RECORDS_PER_STREAM`]) that leaves no room is NOT one of these: it is
/// reported per record (a `None` record id), because `PutRecordBatch` answers a record it
/// could not store with that record's own entry rather than failing the batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamRefusal {
    /// No stream of that name exists.
    NoSuchStream,
    /// The stream exists but is not `ACTIVE`; carries the status it is in.
    NotActive(&'static str),
}

/// Buffer `data` under a freshly minted `RecordId` and answer that id, or `None` when the
/// buffer budget leaves no room for it (in which case nothing is stored and no id is
/// minted).
///
/// The id is an opaque token, unique for the life of this emulator because the
/// service-wide counter is part of it.
fn push_record(
    stream: &mut DeliveryStream,
    counter: &mut u64,
    buffered_bytes: &mut usize,
    data: Vec<u8>,
    now: SystemTime,
) -> Option<String> {
    // Each record is at most the modelled 1,024,000 bytes and each budget is far below
    // `usize::MAX`, so these sums cannot overflow.
    if stream.records.len() >= MAX_BUFFERED_RECORDS_PER_STREAM
        || stream.buffered_bytes + data.len() > MAX_BUFFERED_BYTES_PER_STREAM
        || *buffered_bytes + data.len() > MAX_BUFFERED_BYTES
    {
        return None;
    }

    *counter += 1;
    let nanos = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_nanos())
        .unwrap_or_default();
    let record_id = format!("{nanos:032x}{counter:016x}");
    stream.buffered_bytes += data.len();
    *buffered_bytes += data.len();
    stream.records.push(StoredRecord {
        record_id: record_id.clone(),
        data,
        received_at: now,
    });
    Some(record_id)
}

/// The Region a request was signed for, which is the Region a minted ARN names.
///
/// An unsigned request, or one whose credential scope carries a Region the ARN grammar
/// (`[a-zA-Z0-9-]+`, and short enough to keep the ARN inside its modelled 512-character
/// bound) cannot hold, falls back to the emulator's default Region.
///
/// Lives here, beside the state the ARNs are stored in, so every operation that has to
/// mint or match one uses the same rule.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    const DEFAULT_REGION: &str = "us-east-1";
    /// The longest Region token this accepts: every published Region name is far shorter.
    const MAX_REGION_LEN: usize = 32;
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= MAX_REGION_LEN
                && region
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsFirehoseFirehose20150804State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsFirehoseFirehose20150804State>)
}
