// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-firehose-firehose-20150804 service.
//!
//! One name-keyed register of Firehose streams, plus the service-level counter that
//! mints record ids. Everything an operation answers is computed from here.
//!
//! ## Clock-owed transitions
//!
//! `CreateDeliveryStream`, `DeleteDeliveryStream` and the two encryption calls are
//! asynchronous: the model's own documentation says a created stream starts in `CREATING`
//! and reaches `ACTIVE` later, that a deleted stream goes to `DELETING` and is removed
//! later, and that enabling SSE sets the encryption status to `ENABLING` "and then to
//! ENABLED" (disabling it, `DISABLING` and then `DISABLED`). No span is
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
    DeliveryStreamEncryptionConfigurationInput, ElasticsearchDestinationConfiguration,
    ExtendedS3DestinationConfiguration, HttpEndpointDestinationConfiguration,
    IcebergDestinationConfiguration, ProcessingConfiguration, RedshiftDestinationConfiguration,
    S3DestinationConfiguration, SnowflakeDestinationConfiguration, SourceDescription,
    SplunkDestinationConfiguration, KEY_TYPE_VALUES,
};

/// How long after the starting call a clock-owed stream transition
/// (`CREATING` -> `ACTIVE`, `DELETING` -> removed, `ENABLING` -> `ENABLED`,
/// `DISABLING` -> `DISABLED`) comes due.
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

/// The most tags ONE stream carries.
///
/// Documented, not chosen: "Each Firehose stream can have up to 50 tags"
/// (`TagDeliveryStream`) — the same ceiling as the modelled `TagDeliveryStreamInputTagList`
/// and `ListTagsForDeliveryStreamOutputTagList` length and as `Limit`'s range.
pub const MAX_TAGS_PER_STREAM: usize = 50;

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

/// The window the combined encryption-call limit is counted over.
///
/// Documented, not chosen: "The StartDeliveryStreamEncryption and
/// StopDeliveryStreamEncryption operations have a combined limit of 25 calls per Firehose
/// stream per 24 hours" (`StartDeliveryStreamEncryption`).
pub const ENCRYPTION_CALL_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// How many `StartDeliveryStreamEncryption` / `StopDeliveryStreamEncryption` calls one
/// stream takes inside [`ENCRYPTION_CALL_WINDOW`] — the 25 of that same sentence.
pub const MAX_ENCRYPTION_CALLS_PER_WINDOW: usize = 25;

/// The `KeyType` a request that names none asks for: "The default setting is
/// AWS_OWNED_CMK" (`DeliveryStreamEncryptionConfigurationInput$KeyType`).
pub const DEFAULT_KEY_TYPE: &str = "AWS_OWNED_CMK";

/// The `DeliveryStreamType` whose source is Direct PUT: "Provider applications access the
/// Firehose stream directly." SSE can be enabled only on a stream of this type — "You can
/// enable SSE for a Firehose stream only if it's a Firehose stream that uses DirectPut as
/// its source" (`StartDeliveryStreamEncryption`).
pub const DIRECT_PUT_STREAM_TYPE: &str = "DirectPut";

/// The modelled `ProcessorType` values that make up Firehose's CloudWatch Logs
/// decompression pipeline: `Decompression`, which inflates the gzip payload CloudWatch
/// Logs delivers, and `CloudWatchLogProcessing`, which extracts the log events out of
/// that payload's `DataMessage` envelope. Both are values of `ProcessorType`
/// (`crate::ops::types::PROCESSOR_TYPE_VALUES`) and neither can process anything a
/// CloudWatch Logs producer did not send, which is why a stream whose data processing is
/// enabled with one of them takes data from CloudWatch Logs ONLY: "Only requests from
/// CloudWatch Logs are supported when CloudWatch Logs decompression is enabled"
/// (`InvalidSourceException`).
pub const CLOUDWATCH_LOGS_PROCESSOR_TYPES: &[&str] = &["CloudWatchLogProcessing", "Decompression"];

/// The `DeliveryStreamEncryptionStatus` a started enable is in while it is in flight:
/// "Firehose first sets the encryption status of the stream to ENABLING".
pub const ENCRYPTION_ENABLING: &str = "ENABLING";

/// The `DeliveryStreamEncryptionStatus` the clock owes a started enable: "and then to
/// ENABLED".
pub const ENCRYPTION_ENABLED: &str = "ENABLED";

/// The `DeliveryStreamEncryptionStatus` a started disable is in while it is in flight:
/// "Firehose first sets the encryption status of the stream to DISABLING".
pub const ENCRYPTION_DISABLING: &str = "DISABLING";

/// The `DeliveryStreamEncryptionStatus` the clock owes a started disable: "and then to
/// DISABLED".
pub const ENCRYPTION_DISABLED: &str = "DISABLED";

/// The `DeliveryStreamEncryptionStatus` a disable that failed leaves behind: encryption is
/// still in force, so a further `StopDeliveryStreamEncryption` has something to disable.
/// Modelled (`DeliveryStreamEncryptionStatus`) but unreachable here, because nothing in
/// this emulator fails an encryption change.
pub const ENCRYPTION_DISABLING_FAILED: &str = "DISABLING_FAILED";

/// The `DeliveryStreamEncryptionStatus` values that mean an SSE change is still in
/// flight, so another encryption call on the same stream is refused.
///
/// Named from the two transient statuses above so the set and the pair cannot drift.
pub const IN_FLIGHT_ENCRYPTION_STATUSES: &[&str] = &[ENCRYPTION_ENABLING, ENCRYPTION_DISABLING];

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
    /// `CREATING` -> `ACTIVE` (and `ENABLING` -> `ENABLED` for encryption enabled at
    /// create, which settles with the creation it rode along with).
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

impl DestinationConfigKind {
    /// The data-processing configuration this destination carries, whatever variant it is.
    ///
    /// `S3DestinationConfiguration` is the one destination the model gives no
    /// `ProcessingConfiguration` member, so a plain Amazon S3 destination has none.
    pub fn processing_configuration(&self) -> Option<&ProcessingConfiguration> {
        match self {
            Self::S3(_) => None,
            Self::ExtendedS3(config) => config.processing_configuration.as_ref(),
            Self::Redshift(config) => config.processing_configuration.as_ref(),
            Self::Elasticsearch(config) => config.processing_configuration.as_ref(),
            Self::AmazonOpenSearchService(config) => config.processing_configuration.as_ref(),
            Self::Splunk(config) => config.processing_configuration.as_ref(),
            Self::HttpEndpoint(config) => config.processing_configuration.as_ref(),
            Self::Snowflake(config) => config.processing_configuration.as_ref(),
            Self::AmazonOpenSearchServerless(config) => config.processing_configuration.as_ref(),
            Self::Iceberg(config) => config.processing_configuration.as_ref(),
        }
    }
}

/// Whether `processing` turns on a processor that only a CloudWatch Logs producer can
/// feed ([`CLOUDWATCH_LOGS_PROCESSOR_TYPES`]).
///
/// A configuration whose `Enabled` is not `true` processes nothing — "Enables or disables
/// data processing" (`ProcessingConfiguration$Enabled`) — so the processors it lists are
/// not in force and gate nothing.
fn enables_cloudwatch_logs_processing(processing: &ProcessingConfiguration) -> bool {
    processing.enabled == Some(true)
        && processing.processors.iter().flatten().any(|processor| {
            CLOUDWATCH_LOGS_PROCESSOR_TYPES.contains(&processor.r#type.as_str())
        })
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
    /// The stream's SSE state, as the last encryption call left it. `None` for a stream
    /// SSE was never configured on, which `DescribeDeliveryStream` reports as the
    /// documented disabled state ([`disabled_encryption`]).
    pub encryption: Option<DeliveryStreamEncryptionConfiguration>,
    /// The `DeliveryStreamEncryptionStatus` the clock owes this stream's encryption
    /// configuration, with the instant it comes due. Enabling and disabling SSE are
    /// asynchronous, so the call that starts one leaves the transient status (`ENABLING`
    /// / `DISABLING`) behind and the clock settles it.
    pub pending_encryption: Option<(String, SystemTime)>,
    /// When each `StartDeliveryStreamEncryption` / `StopDeliveryStreamEncryption` call
    /// landed on this stream, oldest first, so their combined windowed limit is counted
    /// from state. Entries older than [`ENCRYPTION_CALL_WINDOW`] are dropped by
    /// [`Self::materialise_due`].
    pub encryption_calls: Vec<SystemTime>,
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

    /// Whether server-side encryption is in force for the data this stream takes right
    /// now, which is what `PutRecord`/`PutRecordBatch` report as `Encrypted`. Only a
    /// settled `ENABLED` encrypts: "You can continue to read and write data to your
    /// Firehose stream while the encryption status is ENABLING, but the data is not
    /// encrypted."
    fn is_encrypted(&self) -> bool {
        self.encryption
            .as_ref()
            .is_some_and(|encryption| encryption.status.as_deref() == Some(ENCRYPTION_ENABLED))
    }

    /// Whether this stream takes data from CloudWatch Logs only, because one of its
    /// destinations has data processing enabled with a processor of the CloudWatch Logs
    /// decompression pipeline ([`CLOUDWATCH_LOGS_PROCESSOR_TYPES`]).
    ///
    /// A property of the stream's configuration, not of any one payload: every direct
    /// `PutRecord` / `PutRecordBatch` to such a stream is refused with the modelled
    /// `InvalidSourceException`, whatever it carries.
    pub fn takes_cloudwatch_logs_only(&self) -> bool {
        self.destinations.iter().any(|destination| {
            destination
                .config
                .processing_configuration()
                .is_some_and(enables_cloudwatch_logs_processing)
        })
    }

    /// The `DeliveryStreamEncryptionStatus` this stream's SSE is in, if SSE was ever
    /// configured on it.
    pub fn encryption_status(&self) -> Option<&str> {
        self.encryption
            .as_ref()
            .and_then(|encryption| encryption.status.as_deref())
    }

    /// Whether an SSE change this stream started has yet to settle, in which case another
    /// encryption call on it is refused as `ResourceInUseException`.
    pub fn encryption_change_in_flight(&self) -> bool {
        self.encryption_status()
            .is_some_and(|status| IN_FLIGHT_ENCRYPTION_STATUSES.contains(&status))
    }

    /// Charge one `StartDeliveryStreamEncryption` / `StopDeliveryStreamEncryption` call
    /// against this stream's windowed budget, answering `false` — and charging nothing —
    /// when [`MAX_ENCRYPTION_CALLS_PER_WINDOW`] have already landed inside
    /// [`ENCRYPTION_CALL_WINDOW`].
    pub fn charge_encryption_call(&mut self, now: SystemTime) -> bool {
        if self.encryption_calls.len() >= MAX_ENCRYPTION_CALLS_PER_WINDOW {
            return false;
        }
        self.encryption_calls.push(now);
        true
    }

    /// Start an asynchronous SSE change: `configuration` is what a caller sees now (its
    /// status transient) and `settled` is the status the clock owes it.
    pub fn begin_encryption_change(
        &mut self,
        configuration: DeliveryStreamEncryptionConfiguration,
        settled: &str,
        now: SystemTime,
    ) {
        self.encryption = Some(configuration);
        self.pending_encryption = Some((settled.to_string(), now + TRANSITION_DELAY));
        self.last_update_timestamp = now;
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

        // The windowed encryption-call budget frees up only with the clock, so dropping
        // the calls that have aged out of the window is owed at `now` too.
        self.encryption_calls.retain(|at| {
            !matches!(now.duration_since(*at), Ok(age) if age >= ENCRYPTION_CALL_WINDOW)
        });

        // An SSE change that has had its span settles to the status it was owed.
        match self.pending_encryption.as_ref() {
            Some((settled, due)) if now >= *due => {
                let settled = settled.clone();
                if let Some(encryption) = self.encryption.as_mut() {
                    encryption.status = Some(settled);
                }
                self.pending_encryption = None;
            }
            _ => {}
        }

        match self.pending {
            Some((PendingTransition::Activate, due)) if now >= due => {
                self.status = StreamStatus::Active;
                self.pending = None;
                if let Some(encryption) = self.encryption.as_mut() {
                    if encryption.status.as_deref() == Some(ENCRYPTION_ENABLING) {
                        encryption.status = Some(ENCRYPTION_ENABLED.to_string());
                    }
                }
                true
            }
            Some((PendingTransition::Remove, due)) if now >= due => false,
            _ => true,
        }
    }
}

/// The SSE state `DescribeDeliveryStream` reports for a stream SSE was never configured
/// on: the documented disabled status and nothing else — with SSE off there is no CMK, so
/// `KeyType` and `KeyARN` are unset ("If KeyType is Amazon Web Services_OWNED_CMK,
/// DeliveryStreamEncryptionConfiguration doesn't contain a value for KeyARN").
///
/// A stream ALWAYS has an SSE state to report: `DeliveryStreamEncryptionConfiguration` is
/// "the server-side encryption (SSE) status for the delivery stream ... You can get
/// DeliveryStreamEncryptionConfiguration by invoking the DescribeDeliveryStream
/// operation", and [`ENCRYPTION_DISABLED`] is the modelled status for "not encrypted".
pub fn disabled_encryption() -> DeliveryStreamEncryptionConfiguration {
    DeliveryStreamEncryptionConfiguration {
        key_arn: None,
        key_type: None,
        status: Some(ENCRYPTION_DISABLED.to_string()),
        failure_description: None,
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
        // A stream configured for the CloudWatch Logs decompression pipeline takes data
        // from CloudWatch Logs only; an API put is by definition not that producer.
        if stream.takes_cloudwatch_logs_only() {
            return Err(StreamRefusal::SourceNotCloudWatchLogs);
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
    /// The stream only takes data from CloudWatch Logs
    /// ([`DeliveryStream::takes_cloudwatch_logs_only`]), and an API put is not that
    /// producer: the modelled `InvalidSourceException`.
    SourceNotCloudWatchLogs,
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

/// Why Firehose cannot use the `DeliveryStreamEncryptionConfigurationInput` a caller
/// supplied, as [`check_encryption_configuration_input`] judges it.
///
/// Each variant names the modelled error the caller reports it with:
/// `CreateDeliveryStream` and `StartDeliveryStreamEncryption` both take this input and
/// both model those two errors, so the rules live here, once, beside the state they are
/// checked before writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncryptionConfigRefusal {
    /// The configuration itself does not hold together: `InvalidArgumentException`,
    /// "The specified input parameter has a value that is not valid."
    InvalidArgument(String),
    /// KMS cannot serve the named CMK: `InvalidKMSResourceException`, whose `Code`
    /// carries the KMS exception type Firehose relays.
    InvalidKmsResource { code: String, message: String },
}

/// Check a requested SSE configuration against the rules Firehose documents for it, with
/// no regard for which operation asked.
///
/// `region` is the Region the request was signed for — the only Region whose CMKs a
/// stream created by it can reach.
pub fn check_encryption_configuration_input(
    requested: &DeliveryStreamEncryptionConfigurationInput,
    region: &str,
) -> Result<(), EncryptionConfigRefusal> {
    if !KEY_TYPE_VALUES.contains(&requested.key_type.as_str()) {
        return Err(EncryptionConfigRefusal::InvalidArgument(format!(
            "KeyType '{}' is not valid; valid values are {}",
            requested.key_type,
            KEY_TYPE_VALUES.join(", ")
        )));
    }
    // "If you set KeyType to CUSTOMER_MANAGED_CMK, you must specify the Amazon Resource
    // Name (ARN) of the CMK. If you set KeyType to AWS_OWNED_CMK, Firehose uses a
    // service-account CMK."
    match (requested.key_type.as_str(), requested.key_arn.as_ref()) {
        ("CUSTOMER_MANAGED_CMK", None) => {
            return Err(EncryptionConfigRefusal::InvalidArgument(
                "KeyARN is required when KeyType is CUSTOMER_MANAGED_CMK".to_string(),
            ))
        }
        ("AWS_OWNED_CMK", Some(_)) => {
            return Err(EncryptionConfigRefusal::InvalidArgument(
                "KeyARN cannot be specified when KeyType is AWS_OWNED_CMK".to_string(),
            ))
        }
        _ => {}
    }
    // A CMK is reachable only from its own Region, so one named in another Region cannot
    // be used: KMS answers NotFoundException, which Firehose reports as
    // `InvalidKMSResourceException` ("Firehose throws this exception when an attempt to
    // put records or to start or stop Firehose stream encryption fails. This happens when
    // the KMS service throws one of the following exception types: AccessDeniedException,
    // InvalidStateException, DisabledException, or NotFoundException."). The ARN grammar
    // itself is modelled, so the boundary has already checked it; the Region is the part
    // this emulator can judge.
    if let Some(key_arn) = requested.key_arn.as_ref() {
        let key_region = key_arn.split(':').nth(3).unwrap_or_default();
        if !key_region.is_empty() && key_region != region {
            return Err(EncryptionConfigRefusal::InvalidKmsResource {
                code: "NotFoundException".to_string(),
                message: format!(
                    "the KMS key {key_arn} is in Region {key_region} and cannot be used \
                     by a Firehose stream in Region {region}"
                ),
            });
        }
    }
    Ok(())
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
