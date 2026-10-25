// SPDX-License-Identifier: CC0-1.0

//! Delivery of a Firehose stream's buffer to its destination.
//!
//! `PutRecord` / `PutRecordBatch` only ACCEPT data: "Firehose buffers records before
//! delivering them to the destination" (`PutRecord`). This module is the other half —
//! what takes the records a stream's buffer holds, once that buffer is due, and writes
//! them into the Amazon S3 location the stream's CURRENT destination configuration names.
//! The one object store in this emulator is the Amazon S3 service's own, so a delivered
//! object is written there and is then read back with `ListObjectsV2` / `GetObject` like
//! any other object.
//!
//! What a delivery is made of, each part read from the destination configuration as it
//! stands at the moment of the flush (so an `UpdateDestination` that repoints `BucketARN`
//! sends every later delivery to the new bucket):
//!
//! * WHEN — `BufferingHints`, whose `SizeInMBs` and `IntervalInSeconds` decide when a
//!   buffer is flushed ([`delivery_window`]).
//! * WHERE — the bucket of `BucketARN`, under `Prefix` with the automatic
//!   `YYYY/MM/DD/HH` expansion ("The \"YYYY/MM/DD/HH\" time format prefix is
//!   automatically used for delivered Amazon S3 files" — `Prefix`), or under the prefix
//!   the enabled `DynamicPartitioningConfiguration`'s partition keys derive
//!   ([`expand_prefix`]), and `ErrorOutputPrefix` for a record whose partition keys could
//!   not be derived ("A prefix that Firehose evaluates and adds to failed records before
//!   writing them to S3" — `ErrorOutputPrefix`).
//! * WHAT — the buffered records' data, concatenated in arrival order, encoded as
//!   `CompressionFormat` asks ([`compressed`]).
//!
//! ## The clock
//!
//! A flush is a clock-owed change like any other in this crate: [`take_due`] applies only
//! what the stored due instant has ALREADY made due, so a caller reading the destination
//! before that instant sees the data still buffered, and no operation sleeps for it.
//! Delivery happens without any further Firehose call, because that is what a producer
//! observes — so [`drive_delivery_clock`] starts one in-process task per store that waits
//! for the earliest due buffer, delivers it and stands down once nothing is owed.
//!
//! ## Lock order
//!
//! This service's store is locked, the records are TAKEN out of it, and the lock is
//! released before the object store is locked: the two locks are never held at once, and
//! the Amazon S3 service never reaches into this one, so a cycle between them is
//! unreachable.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::ops::types::{BufferingHints, ProcessingConfiguration, S3DestinationConfiguration};
use crate::state::{
    service_state, ComAmazonawsFirehoseFirehose20150804State, DeliveryStream,
    DestinationConfigKind, StoredRecord,
};

/// Bytes in one MiB: `BufferingHints$SizeInMBs` buffers "to the specified size, in MiBs".
const BYTES_PER_MIB: usize = 1024 * 1024;

/// `BufferingHints$SizeInMBs` when the configuration names none: "The default value is 5."
const DEFAULT_BUFFER_SIZE_IN_MIB: usize = 5;

/// `BufferingHints$IntervalInSeconds` when the configuration names none: "The default
/// value is 300."
const DEFAULT_BUFFER_INTERVAL: Duration = Duration::from_secs(300);

/// How long after a stream's buffer opens — the instant the oldest record it holds landed
/// — the delivery of that buffer comes due.
///
/// A CHOSEN span, and one the buffering parameters themselves admit: `BufferingHints` is
/// "hints for the buffering to perform before delivering data to the destination. These
/// options are treated as hints, and therefore Firehose might choose to use different
/// values when it is optimal", so a configured interval is the longest a producer may wait
/// and not a promise to wait exactly that long. It is kept short enough for a client to
/// observe a delivery, and never zero, so a caller that reads the destination immediately
/// after a put still sees the record buffered rather than delivered. The configured
/// interval still caps it ([`delivery_window`]).
pub const DELIVERY_SPAN: Duration = Duration::from_millis(600);

/// How long after a stream's buffer opens the delivery comes due once the buffer holds at
/// least `BufferingHints$SizeInMBs` of data.
///
/// The size and the interval are two conditions on one buffer and the FIRST one satisfied
/// flushes it ("Firehose buffers data up to 5 MB or for 5 minutes, whichever condition is
/// satisfied first" — `CreateDeliveryStream`), so a buffer that has reached its size is
/// due sooner than one still filling. Chosen like [`DELIVERY_SPAN`], and never zero for
/// the same reason.
pub const FULL_BUFFER_DELIVERY_SPAN: Duration = Duration::from_millis(250);

/// How long a stream whose buffer could not be delivered at all waits before the delivery
/// is attempted again.
///
/// A CHOSEN span: AWS documents a retry duration only for the destinations whose own
/// `RetryOptions` carry one, and never for how quickly the attempt is repeated. Its job
/// here is that a destination this emulator cannot write to — a `BucketARN` naming a
/// bucket that was never created — costs one attempt per span rather than a spin, while
/// the records stay buffered for the documented [`crate::state::RECORD_RETENTION`] "as it
/// tries to send the records to the destination".
pub const DELIVERY_RETRY_SPAN: Duration = Duration::from_millis(500);

/// The shortest a delivery pump wake-up waits, so a buffer already past its due instant
/// still costs a yield rather than a CPU-bound spin.
const MIN_DELIVERY_WAIT: Duration = Duration::from_millis(5);

/// The longest a delivery pump wake-up waits, so a pump re-reads the store — which a
/// `PutRecord` or an `UpdateDestination` may have changed since — at least this often.
const MAX_DELIVERY_WAIT: Duration = Duration::from_millis(500);

/// How many times one delivery pump wakes up before it stands down.
///
/// A backstop, not a schedule: it bounds the work one put can start, and whatever is still
/// owed when it is reached is delivered by the next pump (every put starts one if none is
/// running).
const MAX_DELIVERY_WAKE_UPS: usize = 2_000;

/// The jq program of one flush gets this long to run over the whole buffer.
///
/// A BACKSTOP, not the bound: a program this emulator runs terminates by construction
/// ([`extract_partition_keys`]), so this is what catches an engine that nevertheless does
/// not come back — the records it did not produce keys for are delivered to the error
/// output prefix, which is where a record whose partition keys could not be determined
/// goes.
const METADATA_EXTRACTION_BUDGET: Duration = Duration::from_secs(2);

/// How often the delivery looks for the result of a running program while waiting out
/// [`METADATA_EXTRACTION_BUDGET`] — an await point, so nothing of the runtime is held
/// while a program runs.
const METADATA_EXTRACTION_POLL: Duration = Duration::from_millis(5);

/// The name the thread a metadata-extraction program runs on carries, so one that outlives
/// its budget is identifiable in a dump.
const METADATA_EXTRACTION_THREAD: &str = "firehose-metadata-extraction";

/// The stack the thread a metadata-extraction program runs on gets.
///
/// The interpreter descends with the program's NESTING, and nothing it runs recurses
/// ([`MAX_PROGRAM_DEPTH`], enforced on the text before the program is parsed), so this is
/// a stack sized for an enforced depth bound rather than a hope.
const METADATA_EXTRACTION_STACK_BYTES: usize = 16 * 1024 * 1024;

/// The deepest a metadata-extraction program may nest, counted over its text BEFORE it is
/// parsed — so no descent over the program has begun when one past this is refused.
///
/// This is the depth bound the interpreter's own descent, and the stack it runs on, are
/// sized for.
const MAX_PROGRAM_DEPTH: usize = 32;

/// The most tokens a metadata-extraction program may carry, counted over its text before
/// it is parsed. Together with [`MAX_PROGRAM_CONSTRUCTIONS`] this is what bounds the VALUE
/// one run can build — see [`admissible_program`].
const MAX_PROGRAM_TOKENS: usize = 128;

/// How many container CONSTRUCTIONS (`{…}`, `[…]`) a metadata-extraction program may
/// carry — indexing and slicing (`.[…]`) are not constructions and are not counted.
///
/// One, because a `MetadataExtractionQuery` produces ONE object of partition keys, and
/// because construction is the only way a jq program makes a value LARGER than the one it
/// was given: a construction holding `n` copies of its input grows it `n`-fold, so a chain
/// of `k` of them grows it `n^k`-fold. With `k` fixed at one and `n` bounded by the token
/// count, the largest value a run can build is bounded — see [`admissible_program`].
const MAX_PROGRAM_CONSTRUCTIONS: usize = 1;

/// The largest record a metadata-extraction program is run over.
///
/// An emulator resource budget, NOT an AWS limit: it is the other half of the memory bound
/// above, since what a run can build is a multiple of what it is given. A record past it
/// has no partition keys derived and goes to the error output, like any other record whose
/// extraction did not succeed. Generous for the JSON document a partition key is read out
/// of, while keeping one run's values at [`MAX_PROGRAM_TOKENS`] times this.
const MAX_EXTRACTED_RECORD_BYTES: usize = 128 * 1024;

/// How many metadata-extraction programs may be running at once in this process.
///
/// A program this emulator runs terminates by construction, so this is not what makes the
/// feature safe — it is what keeps a burst of due buffers from starting an unbounded
/// number of threads at once. The permit is released by the WAITER, on the run's
/// completion AND on the backstop budget expiring, so a run that does not come back cannot
/// hold one.
const MAX_RUNNING_METADATA_EXTRACTIONS: usize = 4;

/// How many runs may be ABANDONED — their backstop budget reached while the engine was
/// still inside one of them — in the life of this process, ever.
///
/// A run this emulator starts terminates by construction, so reaching this means an
/// assumption about the engine is wrong. Past it no program is run at all and every record
/// of a dynamically partitioned stream goes to the error output: the feature fails closed
/// rather than letting a caller start one abandoned thread per put.
const MAX_ABANDONED_EXTRACTIONS: usize = 4;

/// How many programs that overran their budget are remembered. See [`overran_programs`].
const MAX_REMEMBERED_OVERRUNS: usize = 64;

/// The most partition keys one record's extracted metadata may carry.
///
/// An emulator output budget, not an AWS limit: the keys are what a client's own program
/// produced, and they are held in this process and interpolated into an object key, so
/// their number and size are bounded like every other caller-driven growth here.
const MAX_PARTITION_KEYS: usize = 32;

/// The longest one partition key's name or value may be, in bytes — the other half of the
/// budget [`MAX_PARTITION_KEYS`] states. Comfortably inside what an S3 object key can hold
/// (1,024 bytes), so a partition this accepts can still be written.
const MAX_PARTITION_VALUE_BYTES: usize = 256;

/// How many due deliveries ONE call performs before leaving the rest to the next one.
///
/// A store can hold thousands of streams, so without this one put would do the work of
/// every buffer that happens to be due at that instant. What this leaves behind stays
/// buffered and is delivered by the pump's next wake-up.
const MAX_DELIVERIES_PER_CALL: usize = 64;

/// How many metadata-extraction programs are running right now. See
/// [`MAX_RUNNING_METADATA_EXTRACTIONS`].
static RUNNING_METADATA_EXTRACTIONS: ::std::sync::atomic::AtomicUsize =
    ::std::sync::atomic::AtomicUsize::new(0);

/// How many runs have been abandoned in the life of this process. See
/// [`MAX_ABANDONED_EXTRACTIONS`].
static ABANDONED_METADATA_EXTRACTIONS: ::std::sync::atomic::AtomicUsize =
    ::std::sync::atomic::AtomicUsize::new(0);

/// The programs that have already overrun [`METADATA_EXTRACTION_BUDGET`] once, which are
/// never run again: one that did not finish will not finish, and running it per flush would
/// turn one bad configuration into a thread per put. Bounded by
/// [`MAX_REMEMBERED_OVERRUNS`], and each program is at most the modelled 5120 characters.
fn overran_programs() -> &'static ::std::sync::Mutex<::std::collections::BTreeSet<String>> {
    static OVERRAN: ::std::sync::Mutex<::std::collections::BTreeSet<String>> =
        ::std::sync::Mutex::new(::std::collections::BTreeSet::new());
    &OVERRAN
}

/// Whether `query` has already overrun its budget once.
fn has_overrun(query: &str) -> bool {
    overran_programs()
        .lock()
        .unwrap_or_else(::std::sync::PoisonError::into_inner)
        .contains(query)
}

/// Remember that `query` overran its budget, while there is room to.
fn remember_overrun(query: String) {
    let mut remembered = overran_programs()
        .lock()
        .unwrap_or_else(::std::sync::PoisonError::into_inner);
    if remembered.len() < MAX_REMEMBERED_OVERRUNS {
        remembered.insert(query);
    }
}

/// The right to run ONE metadata-extraction program, released by the WAITER — when the
/// run comes back, and also when the backstop budget expires without it. So a run that
/// does not come back cannot hold a permit, and the concurrency limit can never be used up
/// permanently.
struct ExtractionPermit;

impl ExtractionPermit {
    /// Take a permit, or `None` when [`MAX_RUNNING_METADATA_EXTRACTIONS`] are already out.
    fn take() -> Option<Self> {
        use ::std::sync::atomic::Ordering;
        let mut running = RUNNING_METADATA_EXTRACTIONS.load(Ordering::Acquire);
        loop {
            if running >= MAX_RUNNING_METADATA_EXTRACTIONS {
                return None;
            }
            match RUNNING_METADATA_EXTRACTIONS.compare_exchange_weak(
                running,
                running + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(Self),
                Err(held) => running = held,
            }
        }
    }
}

impl Drop for ExtractionPermit {
    fn drop(&mut self) {
        RUNNING_METADATA_EXTRACTIONS.fetch_sub(1, ::std::sync::atomic::Ordering::AcqRel);
    }
}

/// Charge one ABANDONED run against [`MAX_ABANDONED_EXTRACTIONS`], answering whether it
/// fitted — `false` once this process has abandoned as many as it ever will, after which
/// no program is run at all.
fn charge_abandoned_extraction() -> bool {
    ABANDONED_METADATA_EXTRACTIONS.fetch_add(1, ::std::sync::atomic::Ordering::AcqRel)
        < MAX_ABANDONED_EXTRACTIONS
}

/// Whether this process has already abandoned [`MAX_ABANDONED_EXTRACTIONS`] runs.
fn abandoned_budget_spent() -> bool {
    ABANDONED_METADATA_EXTRACTIONS.load(::std::sync::atomic::Ordering::Acquire)
        >= MAX_ABANDONED_EXTRACTIONS
}

/// The `ProcessorType` whose parameters carry the jq program that derives a record's
/// partition keys.
const METADATA_EXTRACTION_PROCESSOR: &str = "MetadataExtraction";

/// The `ProcessorType` that asks for a delimiter between the records of a delivered
/// object: "If you want to add a new line delimiter between records in objects that are
/// delivered to Amazon S3, choose `AppendDelimiterToRecord` as a processor type. You don't
/// have to put a processor parameter when you select `AppendDelimiterToRecord`"
/// (`Processor`).
const APPEND_DELIMITER_PROCESSOR: &str = "AppendDelimiterToRecord";

/// The `ProcessorParameterName` carrying the jq program of a `MetadataExtraction`
/// processor.
const METADATA_EXTRACTION_QUERY_PARAMETER: &str = "MetadataExtractionQuery";

/// The delimiter `AppendDelimiterToRecord` adds between the records of one object: the
/// new line that processor exists to add, and the one `PutRecordBatch` names as the way
/// "to disambiguate the data blobs at the destination".
const RECORD_DELIMITER: &[u8] = b"\n";

/// How a record Firehose could not place is reported in the error output.
///
/// A record that does not reach the destination is not dropped: it is written under the
/// destination's `ErrorOutputPrefix`, wrapped in the error-output document Firehose writes
/// for a failed record — [`error_envelope`] — so a consumer of that prefix can tell what
/// failed and still has the record's own bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct DeliveryFailure {
    /// What the `firehose:error-output-type` namespace of an `ErrorOutputPrefix` expands
    /// to for this failure, and the prefix a destination configuring none gets.
    output_type: &'static str,
    /// The `errorCode` the error-output document names.
    error_code: &'static str,
    /// The `errorMessage` the error-output document names. An example, not a wire
    /// contract: what a consumer keys off is the code beside it.
    error_message: &'static str,
}

/// The failure of a record whose PARTITION KEYS could not be determined: the
/// `MetadataExtraction` program derived none for it (unreadable JSON, a program that does
/// not compile, an output that is not an object of scalars), or the destination's prefix
/// names a key nothing produced — including a `partitionKeyFromLambda` key, which only a
/// Lambda data transformation supplies and this emulator runs none.
const METADATA_EXTRACTION_FAILED: DeliveryFailure = DeliveryFailure {
    output_type: "metadata-extraction-failed",
    error_code: "DynamicPartitioning.MetadataExtractionFailed",
    error_message: "the partition keys of this record could not be determined",
};

/// The `S3BackupMode` value that sends a copy of EVERY record to the backup location,
/// whichever destination kind carries the mode (`S3BackupMode` `Enabled`,
/// `ElasticsearchS3BackupMode` `AllDocuments`, `SplunkS3BackupMode` `AllEvents`,
/// `HttpEndpointS3BackupMode` / `SnowflakeS3BackupMode` / `IcebergS3BackupMode`
/// `AllData`). Under any other mode the Amazon S3 location takes only the records the
/// primary destination rejected, and this emulator delivers to none of those endpoints, so
/// it has no rejected record to write there.
const ALL_RECORDS_BACKUP_MODES: &[&str] = &["Enabled", "AllDocuments", "AllEvents", "AllData"];

/// One Amazon S3 location a stream's buffer is delivered to, with everything the write
/// needs, read off the destination configuration in force at the moment of the flush.
#[derive(Debug, Clone)]
pub struct S3Target {
    /// The bucket named by `BucketARN`, whose resource part IS the bucket name
    /// (`arn:<partition>:s3:::<name>`).
    pub bucket: String,
    /// `Prefix`, verbatim; the expansion happens per delivered object.
    pub prefix: String,
    /// `ErrorOutputPrefix`, verbatim.
    pub error_output_prefix: String,
    /// `BufferingHints`, which decide when this location's buffer is due.
    pub hints: Option<BufferingHints>,
    /// A `CompressionFormat` value, or `None` for the documented default: "The compression
    /// format. If no value is specified, the default is UNCOMPRESSED."
    pub compression_format: Option<String>,
    /// `FileExtension`: "Specify a file extension. It will override the default file
    /// extension."
    pub file_extension: Option<String>,
    /// The jq program of the destination's `MetadataExtraction` processor, when dynamic
    /// partitioning is enabled for this location and such a processor is configured.
    pub metadata_extraction_query: Option<String>,
    /// Whether the records of one object are separated by [`RECORD_DELIMITER`], which the
    /// `AppendDelimiterToRecord` processor asks for.
    pub append_delimiter: bool,
}

/// A stream's buffer, taken out of the store and ready to be written to its destination.
#[derive(Debug)]
pub struct PendingDelivery {
    /// The stream whose buffer this is, which names the delivered objects.
    pub stream: String,
    /// The stream's `VersionId`, which a delivered object's name carries.
    pub version_id: u64,
    /// The buffered records, in arrival order.
    pub records: Vec<StoredRecord>,
    /// Every Amazon S3 location this buffer goes to.
    pub targets: Vec<S3Target>,
}

/// One object a delivery writes.
#[derive(Debug)]
struct DeliveredObject {
    bucket: String,
    key: String,
    body: Vec<u8>,
}

/// The size in bytes at which a buffer with these hints is full, and the longest it is
/// held.
fn buffer_limits(hints: Option<&BufferingHints>) -> (usize, Duration) {
    let size = hints
        .and_then(|hints| hints.size_in_m_bs)
        .and_then(|size| usize::try_from(size).ok())
        .unwrap_or(DEFAULT_BUFFER_SIZE_IN_MIB)
        .saturating_mul(BYTES_PER_MIB);
    let interval = hints
        .and_then(|hints| hints.interval_in_seconds)
        .and_then(|interval| u64::try_from(interval).ok())
        .map_or(DEFAULT_BUFFER_INTERVAL, Duration::from_secs);
    (size, interval)
}

/// How long after it opened a buffer holding `buffered_bytes` under these hints is held
/// before its delivery comes due.
///
/// The emulator's own span ([`DELIVERY_SPAN`], or [`FULL_BUFFER_DELIVERY_SPAN`] once the
/// buffer has reached `SizeInMBs`), capped by the configured `IntervalInSeconds` so a
/// configuration asking for a shorter wait than the emulator's own gets the shorter one,
/// and never shorter than [`FULL_BUFFER_DELIVERY_SPAN`] — `IntervalInSeconds` admits 0,
/// and a zero span would hand a caller a delivery its own put had not yet returned from.
#[must_use]
pub fn delivery_window(hints: Option<&BufferingHints>, buffered_bytes: usize) -> Duration {
    let (size, interval) = buffer_limits(hints);
    let span = if buffered_bytes >= size {
        FULL_BUFFER_DELIVERY_SPAN
    } else {
        DELIVERY_SPAN
    };
    span.min(interval).max(FULL_BUFFER_DELIVERY_SPAN)
}

/// The bucket name a `BucketARN` names, or `None` when the value carries none.
///
/// An S3 bucket ARN has neither account nor Region — `arn:<partition>:s3:::<name>`, the
/// form `BucketARN`'s own modelled pattern requires — so the name is the resource part.
fn bucket_name(bucket_arn: &str) -> Option<&str> {
    let name = bucket_arn.rsplit(':').next().unwrap_or_default();
    (!name.is_empty()).then_some(name)
}

/// The Amazon S3 location a plain `S3DestinationConfiguration` names.
fn plain_target(config: &S3DestinationConfiguration) -> Option<S3Target> {
    Some(S3Target {
        bucket: bucket_name(&config.bucket_arn)?.to_string(),
        prefix: config.prefix.clone().unwrap_or_default(),
        error_output_prefix: config.error_output_prefix.clone().unwrap_or_default(),
        hints: config.buffering_hints.clone(),
        compression_format: config.compression_format.clone(),
        file_extension: None,
        metadata_extraction_query: None,
        append_delimiter: false,
    })
}

/// The jq program the destination's enabled `MetadataExtraction` processor carries, when
/// dynamic partitioning is enabled for it.
///
/// Dynamic partitioning "creates smaller data sets from the streaming data by partitioning
/// it based on partition keys" (`DynamicPartitioningConfiguration`), and the keys come from
/// the `MetadataExtractionQuery` of the data-processing configuration — so a configuration
/// with the mechanism switched off, or with no such processor, derives no keys and the
/// whole buffer goes to one prefix.
fn metadata_extraction_query(
    dynamic_partitioning_enabled: bool,
    processing: Option<&ProcessingConfiguration>,
) -> Option<String> {
    if !dynamic_partitioning_enabled {
        return None;
    }
    let processing = processing?;
    if processing.enabled != Some(true) {
        return None;
    }
    processing
        .processors
        .iter()
        .flatten()
        .filter(|processor| processor.r#type == METADATA_EXTRACTION_PROCESSOR)
        .flat_map(|processor| processor.parameters.iter().flatten())
        .find(|parameter| parameter.parameter_name == METADATA_EXTRACTION_QUERY_PARAMETER)
        .map(|parameter| parameter.parameter_value.clone())
}

/// Whether the destination's data processing asks for a delimiter between the records of
/// one delivered object.
fn appends_delimiter(processing: Option<&ProcessingConfiguration>) -> bool {
    processing.is_some_and(|processing| {
        processing.enabled == Some(true)
            && processing
                .processors
                .iter()
                .flatten()
                .any(|processor| processor.r#type == APPEND_DELIMITER_PROCESSOR)
    })
}

/// Whether an `S3BackupMode` value sends every record to the backup location.
fn backs_up_all_records(mode: Option<&String>) -> bool {
    mode.is_some_and(|mode| ALL_RECORDS_BACKUP_MODES.contains(&mode.as_str()))
}

/// Every Amazon S3 location a destination configuration delivers EVERY ingested record to.
///
/// * A plain or extended Amazon S3 destination is itself such a location.
/// * An Amazon Redshift destination has one too: "Firehose first delivers data to the S3
///   bucket and then issues an Amazon Redshift COPY command"
///   (`CreateDeliveryStream`), so its `S3Configuration` takes every record on the way.
/// * Every other destination kind keeps its `S3Configuration` for BACKUP, and only the
///   mode that backs up all records ([`ALL_RECORDS_BACKUP_MODES`]) makes it a location
///   every record reaches; under the failed-records-only modes it takes what the primary
///   destination rejected, and this emulator delivers to none of those endpoints, so it has
///   nothing to write there.
/// * A destination that backs up to Amazon S3 as well ([`ALL_RECORDS_BACKUP_MODES`] with an
///   `S3BackupConfiguration`) delivers to that location too.
#[must_use]
pub fn s3_targets(config: &DestinationConfigKind) -> Vec<S3Target> {
    let mut targets = Vec::new();
    match config {
        DestinationConfigKind::S3(config) => targets.extend(plain_target(config)),
        DestinationConfigKind::ExtendedS3(config) => {
            if let Some(name) = bucket_name(&config.bucket_arn) {
                let dynamic_partitioning_enabled = config
                    .dynamic_partitioning_configuration
                    .as_ref()
                    .is_some_and(|partitioning| partitioning.enabled == Some(true));
                targets.push(S3Target {
                    bucket: name.to_string(),
                    prefix: config.prefix.clone().unwrap_or_default(),
                    error_output_prefix: config.error_output_prefix.clone().unwrap_or_default(),
                    hints: config.buffering_hints.clone(),
                    compression_format: config.compression_format.clone(),
                    file_extension: config.file_extension.clone(),
                    metadata_extraction_query: metadata_extraction_query(
                        dynamic_partitioning_enabled,
                        config.processing_configuration.as_ref(),
                    ),
                    append_delimiter: appends_delimiter(config.processing_configuration.as_ref()),
                });
            }
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(config.s3_backup_configuration.as_ref().and_then(plain_target));
            }
        }
        DestinationConfigKind::Redshift(config) => {
            targets.extend(plain_target(&config.s3_configuration));
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(config.s3_backup_configuration.as_ref().and_then(plain_target));
            }
        }
        DestinationConfigKind::Elasticsearch(config) => {
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(plain_target(&config.s3_configuration));
            }
        }
        DestinationConfigKind::AmazonOpenSearchService(config) => {
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(plain_target(&config.s3_configuration));
            }
        }
        DestinationConfigKind::Splunk(config) => {
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(plain_target(&config.s3_configuration));
            }
        }
        DestinationConfigKind::HttpEndpoint(config) => {
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(plain_target(&config.s3_configuration));
            }
        }
        DestinationConfigKind::Snowflake(config) => {
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(plain_target(&config.s3_configuration));
            }
        }
        DestinationConfigKind::AmazonOpenSearchServerless(config) => {
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(plain_target(&config.s3_configuration));
            }
        }
        DestinationConfigKind::Iceberg(config) => {
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(plain_target(&config.s3_configuration));
            }
        }
    }
    targets
}

/// Every Amazon S3 location this stream's destinations deliver to right now.
fn stream_targets(stream: &DeliveryStream) -> Vec<S3Target> {
    stream
        .destinations
        .iter()
        .flat_map(|destination| s3_targets(&destination.config))
        .collect()
}

/// When this stream's buffer comes due for delivery, or `None` when nothing is owed — an
/// empty buffer, or a destination with no Amazon S3 location to deliver to.
#[must_use]
pub fn due_at(stream: &DeliveryStream) -> Option<SystemTime> {
    let opened_at = stream.records.first()?.received_at;
    let window = stream_targets(stream)
        .iter()
        .map(|target| delivery_window(target.hints.as_ref(), stream.buffered_bytes))
        .min()?;
    let due = opened_at + window;
    Some(match stream.delivery_retry_at {
        Some(retry_at) if retry_at > due => retry_at,
        _ => due,
    })
}

/// The earliest instant this store owes a delivery at, if any.
#[must_use]
pub fn next_due_at(store: &ComAmazonawsFirehoseFirehose20150804State) -> Option<SystemTime> {
    store.streams.values().filter_map(due_at).min()
}

/// Which deliveries a call to [`deliver_due`] performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extractions {
    /// Perform every due delivery, running a destination's `MetadataExtractionQuery` where
    /// one is configured. What the delivery pump does.
    Run,
    /// Perform only the deliveries that need no metadata-extraction program, leaving the
    /// rest to the pump. What a REQUEST does: a put must not be held while a program
    /// belonging to another stream's buffer runs.
    LeaveToThePump,
}

/// Take the buffer of every stream whose delivery `now` has already made due, at most
/// [`MAX_DELIVERIES_PER_CALL`] of them.
///
/// Writes nothing that was not already owed at `now`: a buffer still inside its window is
/// left exactly as it was, so a reporting operation calling this cannot change what a
/// later caller sees. What this leaves behind — because the cap was reached, or because
/// the caller leaves extraction-bearing deliveries to the pump — stays buffered, exactly
/// where it was, for a later delivery.
pub fn take_due(
    store: &mut ComAmazonawsFirehoseFirehose20150804State,
    now: SystemTime,
    extractions: Extractions,
) -> Vec<PendingDelivery> {
    let mut due = Vec::new();
    let mut freed = 0usize;
    for stream in store.streams.values_mut() {
        if due.len() >= MAX_DELIVERIES_PER_CALL {
            break;
        }
        if due_at(stream).is_none_or(|due_at| due_at > now) {
            continue;
        }
        let targets = stream_targets(stream);
        if targets.is_empty() {
            continue;
        }
        if extractions == Extractions::LeaveToThePump
            && targets
                .iter()
                .any(|target| target.metadata_extraction_query.is_some())
        {
            continue;
        }
        let records = ::std::mem::take(&mut stream.records);
        let bytes: usize = records.iter().map(|record| record.data.len()).sum();
        stream.buffered_bytes = stream.buffered_bytes.saturating_sub(bytes);
        stream.delivery_retry_at = None;
        freed += bytes;
        due.push(PendingDelivery {
            stream: stream.name.clone(),
            version_id: stream.version_id,
            records,
            targets,
        });
    }
    store.buffered_bytes = store.buffered_bytes.saturating_sub(freed);
    due
}

/// Put a delivery that could not be made back in its stream's buffer, ahead of whatever
/// arrived since, and hold the next attempt off until [`DELIVERY_RETRY_SPAN`] has passed.
///
/// A record Firehose cannot deliver is not a record it drops: it keeps trying for the
/// documented retention. The stream being gone (deleted while the delivery was in flight)
/// takes its buffered data with it, so there is nothing to restore.
fn restore(
    store: &mut ComAmazonawsFirehoseFirehose20150804State,
    delivery: PendingDelivery,
    now: SystemTime,
) {
    let Some(stream) = store.streams.get_mut(&delivery.stream) else {
        return;
    };
    let bytes: usize = delivery
        .records
        .iter()
        .map(|record| record.data.len())
        .sum();
    let mut records = delivery.records;
    records.append(&mut stream.records);
    stream.records = records;
    stream.buffered_bytes = stream.buffered_bytes.saturating_add(bytes);
    store.buffered_bytes = store.buffered_bytes.saturating_add(bytes);
    stream.delivery_retry_at = Some(now + DELIVERY_RETRY_SPAN);
}

/// Deliver every buffer this store owes at `now`.
///
/// The one entry point: a handler calls it (so a put also materialises what an earlier
/// buffer was owed), and so does the delivery pump. `extractions` is what separates the
/// two — see [`Extractions`].
pub async fn deliver_due<Eng: Send + Sync + 'static>(
    engine: &Eng,
    now: SystemTime,
    extractions: Extractions,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let pending = {
        let mut store = state.lock();
        store.materialise_due(now);
        take_due(&mut store, now, extractions)
    };
    for delivery in pending {
        // A delivery this emulator could not WORK OUT right now — an encoder that refused
        // the body — is owed again, exactly like one the destination could not take:
        // nothing of it is written and nothing of it is lost.
        let Some(objects) = objects_of(&delivery, now).await else {
            let mut store = state.lock();
            restore(&mut store, delivery, now);
            continue;
        };
        if write_objects(engine, objects) {
            continue;
        }
        let mut store = state.lock();
        restore(&mut store, delivery, now);
    }
}

/// Write `objects` into the one object store, answering whether the destination took
/// them.
///
/// `false` means NOTHING was written and the delivery is owed again: the bucket a
/// `BucketARN` names was never created, or the object store refused every write. A write
/// the store refuses once some object of the same delivery is already in it is reported
/// rather than retried, because retrying would deliver the written object twice.
fn write_objects<Eng: Send + Sync + 'static>(engine: &Eng, objects: Vec<DeliveredObject>) -> bool {
    if objects.is_empty() {
        return true;
    }
    let Some(state) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        ::tracing::warn!("the object store is not reachable from this engine, so nothing was delivered");
        return false;
    };
    let mut store = state.lock();
    let mut written = 0usize;
    for object in objects {
        let stored = ::service_com_amazonaws_s3_amazons3::state::Object {
            // The entity tag is computed from the bytes the object is stored with,
            // exactly as it is for an object written through `PutObject`.
            etag: ::service_com_amazonaws_s3_amazons3::state::object_etag(&object.body),
            body: object.body,
            version_id: None,
            last_modified: SystemTime::now(),
            storage_class: ::service_com_amazonaws_s3_amazons3::state::DEFAULT_STORAGE_CLASS
                .to_string(),
            content_type: None,
            cache_control: None,
            content_disposition: None,
            content_encoding: None,
            content_language: None,
            expires: None,
            website_redirect_location: None,
            metadata: BTreeMap::new(),
            tags: Vec::new(),
            encryption: ::service_com_amazonaws_s3_amazons3::state::ObjectEncryption::default(),
        };
        match store.put_object(&object.bucket, &object.key, stored) {
            Some(
                ::service_com_amazonaws_s3_amazons3::state::ObjectWrite::Created
                | ::service_com_amazonaws_s3_amazons3::state::ObjectWrite::Replaced,
            ) => {
                written += 1;
            }
            outcome => ::tracing::warn!(
                bucket = %object.bucket,
                key = %object.key,
                ?outcome,
                "the destination bucket did not take a delivered object"
            ),
        }
    }
    written > 0
}

/// The objects one delivery writes: one per destination location and, where dynamic
/// partitioning is enabled, one per distinct set of partition keys plus one per reason a
/// record could not be placed.
///
/// `None` when the delivery could not be worked out at all right now and is owed again
/// (see [`grouped`]), so the caller puts it back rather than writing part of it.
async fn objects_of(delivery: &PendingDelivery, now: SystemTime) -> Option<Vec<DeliveredObject>> {
    let mut objects = Vec::new();
    for target in &delivery.targets {
        // The prefix each object of this location is written under, with the records that
        // go in it. A group whose own prefix cannot be resolved — the prefix names a
        // partition key the query derived no value for — joins the failed records, which
        // is what the error output prefix is for: it is never dropped.
        let mut writing: Vec<(String, Vec<&StoredRecord>)> = Vec::new();
        let mut failed: BTreeMap<DeliveryFailure, Vec<&StoredRecord>> = BTreeMap::new();
        for group in grouped(target, &delivery.records).await? {
            match group.keys {
                GroupKeys::Derived(keys) => {
                    match expand_prefix(&target.prefix, &keys, None, now) {
                        Ok(prefix) => writing.push((prefix, group.records)),
                        Err(failure) => failed.entry(failure).or_default().extend(group.records),
                    }
                }
                GroupKeys::Failed(failure) => {
                    failed.entry(failure).or_default().extend(group.records);
                }
            }
        }
        // Each object of this location, with the bytes it carries: a delivered one holds
        // its records' data, and an error-output one holds the error document of each
        // record that could not be placed.
        let mut bodies: Vec<(String, Vec<u8>)> = writing
            .into_iter()
            .map(|(prefix, records)| (prefix, payload(&records, target)))
            .collect();
        for (failure, records) in failed {
            // An error output prefix this emulator cannot resolve is not a record it
            // drops: nothing of the delivery is written and the whole of it is owed
            // again, so the records keep their documented retention.
            let Ok(prefix) = error_output_prefix(target, failure, now) else {
                ::tracing::warn!(
                    bucket = %target.bucket,
                    error_code = %failure.error_code,
                    "the destination's error output prefix could not be resolved, so this \
                     delivery was left for a later one"
                );
                return None;
            };
            bodies.push((prefix, error_payload(&records, failure, now)));
        }
        for (prefix, body) in bodies {
            let name = object_name(delivery, now, objects.len());
            // An encoder that refused the body is not a record this delivery drops: the
            // whole delivery is owed again instead.
            let (body, extension) = compressed(body, target)?;
            objects.push(DeliveredObject {
                bucket: target.bucket.clone(),
                key: format!("{prefix}{name}{extension}"),
                body,
            });
        }
    }
    Some(objects)
}

/// The expanded `ErrorOutputPrefix` a record whose delivery failed is written under.
///
/// "A prefix that Firehose evaluates and adds to failed records before writing them to S3"
/// (`ErrorOutputPrefix`) — with none configured, the error output type is itself that
/// prefix, so a failed record is never written where a delivered one goes.
fn error_output_prefix(
    target: &S3Target,
    failure: DeliveryFailure,
    now: SystemTime,
) -> Result<String, DeliveryFailure> {
    let configured = if target.error_output_prefix.is_empty() {
        format!("{}/", failure.output_type)
    } else {
        target.error_output_prefix.clone()
    };
    expand_prefix(
        &configured,
        &BTreeMap::new(),
        Some(failure.output_type),
        now,
    )
}

/// What an object's records have in common: the partition keys they share, or how their
/// delivery failed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum GroupKeys {
    /// The partition keys every record of the group derived. EMPTY for a destination
    /// without dynamic partitioning, where the whole buffer is one group.
    Derived(BTreeMap<String, String>),
    /// How the delivery of every record in the group failed.
    Failed(DeliveryFailure),
}

/// One object's worth of records.
struct RecordGroup<'r> {
    keys: GroupKeys,
    records: Vec<&'r StoredRecord>,
}

/// The records of one delivery, grouped into the objects they are written as.
///
/// Without dynamic partitioning there is one group: "Firehose buffers records before
/// delivering them to the destination", and the buffer is one object. With it, the records
/// are grouped by the partition keys the `MetadataExtractionQuery` derives, so records
/// carrying distinct values land under distinct prefixes — and the records it derived none
/// for are their own group, delivered to the error output prefix.
///
/// `None` when the extraction could not be STARTED, which is not a verdict on the records:
/// the delivery is owed again instead of being written as failures.
async fn grouped<'r>(
    target: &S3Target,
    records: &'r [StoredRecord],
) -> Option<Vec<RecordGroup<'r>>> {
    let Some(query) = target.metadata_extraction_query.clone() else {
        return Some(vec![RecordGroup {
            keys: GroupKeys::Derived(BTreeMap::new()),
            records: records.iter().collect(),
        }]);
    };
    let keys = partition_keys(query, records).await?;
    // Grouped in a `BTreeMap` so the objects of one flush are built in a deterministic
    // order whatever order the records arrived in.
    let mut groups: BTreeMap<GroupKeys, Vec<&StoredRecord>> = BTreeMap::new();
    for (record, keys) in records.iter().zip(keys) {
        groups.entry(keys).or_default().push(record);
    }
    Some(
        groups
            .into_iter()
            .map(|(keys, records)| RecordGroup { keys, records })
            .collect(),
    )
}

/// The partition keys `query` derives from each record, in the records' own order.
///
/// A record the program produced no object of scalars for — unreadable JSON, a program
/// this emulator will not run or cannot compile, an output that is not an object, a value
/// that is not a scalar — is one whose metadata extraction failed, and carries that
/// `error-output-type` instead. Every answer here is one of those two: nothing is left
/// pending on a program, so no caller can wedge the delivery of a stream by configuring
/// one.
///
/// HOW THE RUN IS BOUNDED. The program is run by [`jaq_core`], a maintained jq
/// implementation, because `JsonParsingEngine` names jq (`JQ-1.6`) and a jq program means
/// what jq says it means and not what a hand-written subset of it would. It is
/// CLIENT-supplied, so:
///
/// * STEPS and MEMORY — [`extract_partition_keys`] registers NO library function at all,
///   so the program has no generating or recursive function to call, and
///   [`admissible_program`] refuses the constructs that re-apply an expression or grow a
///   value without bound. One run's values are then at most [`MAX_PROGRAM_TOKENS`] times
///   [`MAX_EXTRACTED_RECORD_BYTES`], and its steps a straight-line walk of the same size:
///   both bounded by the program and the data rather than by a clock. The arithmetic is set
///   out on [`admissible_program`].
/// * DEPTH — [`MAX_PROGRAM_DEPTH`], checked on the TEXT before anything parses it, and the
///   thread's stack is sized for that depth ([`METADATA_EXTRACTION_STACK_BYTES`]).
/// * OUTPUT — only the FIRST output of each record is read, and what it may carry is
///   capped ([`scalar_object`]).
/// * HOST — the engine is given no library functions, no inputs, no variables and no
///   module path, so there is no `env`, `input`, `include`, `debug` or any other
///   host-reaching native in it: a program can reach nothing but the record handed to it.
/// * CANCELLATION — the run reads a flag this waiter sets, between records, and stops
///   there; a run bounded as above cannot be inside one record for long, so this is what
///   ends a run the waiter has given up on rather than leaving it to finish the buffer.
/// * BACKSTOP — the run gets only [`METADATA_EXTRACTION_BUDGET`] of wall clock, on a
///   thread of its own that is never joined (so neither a request nor the shutdown waits
///   on it) and whose permit the WAITER releases. A run that had to be cancelled is charged
///   against [`MAX_ABANDONED_EXTRACTIONS`] for the life of the process, and the program
///   that caused it is never run again, so the feature fails closed instead of letting a
///   caller start one such run per put.
async fn partition_keys(query: String, records: &[StoredRecord]) -> Option<Vec<GroupKeys>> {
    let data: Vec<Vec<u8>> = records.iter().map(|record| record.data.clone()).collect();
    let failed = vec![GroupKeys::Failed(METADATA_EXTRACTION_FAILED); data.len()];
    if let Err(refusal) = admissible_program(&query) {
        ::tracing::warn!(%refusal, "the metadata extraction query is not one this emulator runs");
        return Some(failed);
    }
    if has_overrun(&query) {
        ::tracing::warn!(
            "this metadata extraction query has already overrun its budget once, so it was \
             not run again"
        );
        return Some(failed);
    }
    if abandoned_budget_spent() {
        ::tracing::warn!(
            "this process has abandoned as many metadata extraction runs as it ever will, so \
             no further query is run"
        );
        return Some(failed);
    }
    let Some(permit) = ExtractionPermit::take() else {
        ::tracing::warn!(
            "too many metadata extraction queries are already running, so this one was not \
             started"
        );
        return Some(failed);
    };
    // The program runs on a thread of its own, with a stack sized for the depth bound and a
    // name that identifies it: never a runtime worker, and never joined, so a program that
    // did not come back cannot hold a request, the delivery loop or the shutdown. The run
    // reads `cancelled` between records, so this waiter can end it.
    let cancelled = ::std::sync::Arc::new(::std::sync::atomic::AtomicBool::new(false));
    let (sender, receiver) = ::std::sync::mpsc::channel();
    let program = query.clone();
    let run_cancelled = ::std::sync::Arc::clone(&cancelled);
    let spawned = ::std::thread::Builder::new()
        .name(METADATA_EXTRACTION_THREAD.to_string())
        .stack_size(METADATA_EXTRACTION_STACK_BYTES)
        .spawn(move || {
            let _ = sender.send(extract_partition_keys(&program, &data, &run_cancelled));
        });
    // The handle is KEPT for as long as this waiter runs, so the run is not forgotten the
    // instant it is started; a synchronous run cannot be aborted from outside, which is
    // what `cancelled` above is for.
    let Ok(handle) = spawned else {
        ::tracing::warn!("a thread for the metadata extraction query could not be started");
        return Some(failed);
    };
    let deadline = ::std::time::Instant::now() + METADATA_EXTRACTION_BUDGET;
    loop {
        match receiver.try_recv() {
            // The permit is released HERE, by this waiter, whichever way the wait ends.
            Ok(keys) => return Some(keys),
            // The run ended without an answer: it panicked, which is the program's own
            // failure and not a defect of the records.
            Err(::std::sync::mpsc::TryRecvError::Disconnected) => {
                ::tracing::warn!("the metadata extraction query could not be run");
                return Some(failed);
            }
            Err(::std::sync::mpsc::TryRecvError::Empty) => {}
        }
        if ::std::time::Instant::now() >= deadline {
            cancelled.store(true, ::std::sync::atomic::Ordering::Release);
            let charged = charge_abandoned_extraction();
            ::tracing::warn!(
                charged,
                finished = handle.is_finished(),
                "the metadata extraction query did not come back inside its budget, so it \
                 was cancelled"
            );
            remember_overrun(query);
            drop(permit);
            return Some(failed);
        }
        ::tokio::time::sleep(METADATA_EXTRACTION_POLL).await;
    }
}

/// Why a metadata-extraction program is not one this emulator runs at all.
///
/// Every check is applied to the TEXT, so no descent over the program has begun when one
/// is refused, and together they are what bound the run's steps, memory and depth
/// ([`partition_keys`]). A program is refused when:
///
/// * it nests deeper than [`MAX_PROGRAM_DEPTH`], or carries more than
///   [`MAX_PROGRAM_TOKENS`] tokens;
/// * it DEFINES a function (`def`), which — with no library function registered — is the
///   one construct left that may call itself, or LOOPS (`reduce`, `foreach`), which re-apply
///   their update expression once per element of the value they are given;
/// * it carries `+` or `*`, the operators that GROW a value in place (`+` concatenates and
///   merges, `*` repeats a string and merges deeply), so repeated application of either
///   doubles a value per step;
/// * it interpolates (`\(…)`), whose contents are an expression this accounting does not
///   read, so brackets, tokens and a definition inside one would be charged nothing;
/// * it carries more than [`MAX_PROGRAM_CONSTRUCTIONS`] container constructions;
/// * it carries a string literal this scanner cannot read as the engine would — one that
///   never closes, or that carries a raw line break jq does not admit — because a scanner
///   that read the text differently from the engine would be shown one program while the
///   engine ran another. A `#` comment runs to the end of its line, exactly as jq reads
///   one, so nothing can be hidden behind one either.
///
/// THE MEMORY BOUND those last three give. With no library function registered there is no
/// generator and no recursion; with no definition and no loop form there is nothing that
/// re-applies an expression, so a program is STRAIGHT-LINE: it either takes the input APART
/// (a path, an index, a slice, a comparison, `if`, `//`, `|`, a binding) or CONSTRUCTS one
/// container from it. A construction holding `n` copies of its input grows it `n`-fold and
/// a chain of `k` constructions grows it `n^k`-fold — so with `k` capped at one and `n`
/// bounded by the token count, the largest value one run can build is at most
/// [`MAX_PROGRAM_TOKENS`] times [`MAX_EXTRACTED_RECORD_BYTES`], and at most
/// [`MAX_RUNNING_METADATA_EXTRACTIONS`] runs hold one at a time. The steps it takes are
/// bounded the same way: a straight-line walk of at most [`MAX_PROGRAM_TOKENS`] tokens over
/// a value of at most that size.
///
/// A program this refuses is a program whose records go to the error output, which is what
/// Firehose does with a query whose metadata extraction does not succeed.
fn admissible_program(query: &str) -> Result<(), &'static str> {
    let mut depth = 0usize;
    let mut deepest = 0usize;
    let mut tokens = 0usize;
    let mut constructions = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut word = String::new();
    // Whether the previous token can be INDEXED into, which is what tells `.foo[0]` and
    // `.["a"]` (an index or a slice) from `[.foo]` (a construction).
    let mut indexable = false;
    // Whether a `#` comment is running: jq comments run to the end of the line, so what
    // follows one on that line is not program text — a scanner that missed them could be
    // shown one program and the engine another.
    let mut in_comment = false;
    for character in query.chars() {
        if in_comment {
            in_comment = character != '\n';
            continue;
        }
        // A `def` inside a string literal defines nothing, and neither does one inside a
        // longer identifier, so the text is read as tokens rather than searched.
        if in_string {
            match character {
                '(' if escaped => return Err("it interpolates"),
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => {
                    in_string = false;
                    indexable = true;
                }
                // A jq string literal does not carry a raw line break (`\n` is how one
                // carries that), so this is text this scanner cannot read as the engine
                // would: refuse rather than guess.
                '\n' | '\r' => return Err("it carries a string this emulator cannot read"),
                _ => {}
            }
            continue;
        }
        if character == '#' {
            in_comment = true;
            continue;
        }
        if character == '"' {
            in_string = true;
            tokens += 1;
            continue;
        }
        if character.is_alphanumeric() || character == '_' {
            word.push(character);
            continue;
        }
        if !word.is_empty() {
            if let Some(refusal) = inadmissible_word(&word) {
                return Err(refusal);
            }
            word.clear();
            tokens += 1;
            indexable = true;
        }
        match character {
            '(' | '[' | '{' => {
                depth += 1;
                deepest = deepest.max(depth);
                tokens += 1;
                // `(` groups rather than constructs, and a `[` right after something
                // indexable is an index or a slice.
                if character == '{' || (character == '[' && !indexable) {
                    constructions += 1;
                }
                indexable = false;
            }
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                tokens += 1;
                indexable = true;
            }
            '+' | '*' => return Err("it carries an operator that grows a value"),
            character if character.is_whitespace() => {}
            '.' | '?' => {
                tokens += 1;
                indexable = true;
            }
            _ => {
                tokens += 1;
                indexable = false;
            }
        }
    }
    if in_string {
        // Text whose string literal never closes is text this scanner cannot read as the
        // engine would, so it is refused rather than admitted on a partial reading.
        return Err("it carries a string this emulator cannot read");
    }
    if let Some(refusal) = inadmissible_word(&word) {
        return Err(refusal);
    }
    if !word.is_empty() {
        tokens += 1;
    }
    if deepest > MAX_PROGRAM_DEPTH {
        return Err("it nests deeper than this emulator reads");
    }
    if tokens > MAX_PROGRAM_TOKENS {
        return Err("it carries more tokens than this emulator reads");
    }
    if constructions > MAX_PROGRAM_CONSTRUCTIONS {
        return Err("it constructs more containers than this emulator runs");
    }
    Ok(())
}

/// Why a jq KEYWORD makes a program inadmissible, or `None` for a word that does not.
///
/// The three core forms whose evaluation is not bounded by the program's own text: a
/// definition may call itself, and `reduce` / `foreach` re-apply their update expression
/// once per element of the value they are given, so one construction inside either is
/// worth an unbounded chain of them.
fn inadmissible_word(word: &str) -> Option<&'static str> {
    match word {
        "def" => Some("it defines a function"),
        "reduce" | "foreach" => Some("it loops over a value"),
        _ => None,
    }
}

/// Compile `query` once and run it over every record. See [`partition_keys`].
///
/// The engine is built DEFAULT-DENY: `Loader::new([])` gives it no definition to load and
/// `Compiler::default()` registers no native function, so the only things a program can
/// name are jq's own core constructs over the value handed to it — path access, object and
/// array construction, comparison, `if`, `try`, `|` and `,`; the looping forms core jq also
/// has are refused before this runs ([`admissible_program`]). Nothing that reads the
/// environment, a file, an input stream or another module exists in this engine to be
/// called, and nothing that generates values without bound (`range`, `repeat`, `while`,
/// `until`, `recurse`) does either. A program naming one does not compile, and its records
/// go to the error output.
///
/// `cancelled` is read between records, so a waiter that has given up ends the run rather
/// than leaving it to walk the rest of the buffer.
fn extract_partition_keys(
    query: &str,
    data: &[Vec<u8>],
    cancelled: &::std::sync::atomic::AtomicBool,
) -> Vec<GroupKeys> {
    use ::jaq_core::load::{Arena, File, Loader};
    use ::jaq_core::{Compiler, Ctx, RcIter};
    use ::jaq_json::Val;

    let failed = GroupKeys::Failed(METADATA_EXTRACTION_FAILED);
    let arena = Arena::default();
    let loader = Loader::new([]);
    let Ok(modules) = loader.load(&arena, File { code: query, path: () }) else {
        return vec![failed; data.len()];
    };
    let Ok(filter) = Compiler::<_, ::jaq_core::Native<Val>>::default().compile(modules) else {
        return vec![failed; data.len()];
    };
    data.iter()
        .map(|record| {
            if cancelled.load(::std::sync::atomic::Ordering::Acquire) {
                return failed.clone();
            }
            // The other half of the memory bound: what one run can build is a multiple of
            // what it is given, so what it is given is capped. A record past it has no
            // keys derived, like any other record whose extraction did not succeed.
            if record.len() > MAX_EXTRACTED_RECORD_BYTES {
                return failed.clone();
            }
            let derived = (|| {
                let value: ::serde_json::Value = ::serde_json::from_slice(record).ok()?;
                let inputs = RcIter::new(::std::iter::empty());
                let output = filter
                    .run((Ctx::new([], &inputs), Val::from(value)))
                    .next()?
                    .ok()?;
                scalar_object(&::serde_json::Value::from(output))
            })();
            derived.map_or_else(|| failed.clone(), GroupKeys::Derived)
        })
        .collect()
}

/// `value` as a partition-key map: an object whose every value is a scalar, each rendered
/// as the text a prefix carries it as. Anything else answers `None`.
///
/// The OUTPUT cap on a program a caller supplied: a result past
/// [`MAX_PARTITION_KEYS`] keys, or carrying a value past [`MAX_PARTITION_VALUE_BYTES`], is
/// not a partition this delivery takes — so what a program can make the emulator retain is
/// bounded, like the time it gets.
fn scalar_object(value: &::serde_json::Value) -> Option<BTreeMap<String, String>> {
    let object = value.as_object()?;
    if object.len() > MAX_PARTITION_KEYS {
        return None;
    }
    let mut keys = BTreeMap::new();
    for (name, value) in object {
        let rendered = match value {
            ::serde_json::Value::String(text) => text.clone(),
            ::serde_json::Value::Number(number) => number.to_string(),
            ::serde_json::Value::Bool(flag) => flag.to_string(),
            // A null, an array or an object is not a value a prefix can carry.
            _ => return None,
        };
        if name.len() > MAX_PARTITION_VALUE_BYTES || rendered.len() > MAX_PARTITION_VALUE_BYTES {
            return None;
        }
        keys.insert(name.clone(), rendered);
    }
    Some(keys)
}

/// The data one object carries: its records' blobs in arrival order, with the delimiter
/// the destination's processing asks for between them.
fn payload(records: &[&StoredRecord], target: &S3Target) -> Vec<u8> {
    let mut body = Vec::new();
    for record in records {
        body.extend_from_slice(&record.data);
        if target.append_delimiter {
            body.extend_from_slice(RECORD_DELIMITER);
        }
    }
    body
}

/// The data an ERROR OUTPUT object carries: the error-output document of each record that
/// could not be placed, one per line.
///
/// A record in the error output is not written raw — then nothing would say what failed:
/// Firehose writes a document naming the failure and carrying the record's own bytes, so a
/// consumer of the error prefix can both diagnose and re-drive it. One document per line,
/// so an object holding several is read a document at a time.
fn error_payload(records: &[&StoredRecord], failure: DeliveryFailure, now: SystemTime) -> Vec<u8> {
    let documents: Vec<String> = records
        .iter()
        .map(|record| error_envelope(record, failure, now).to_string())
        .collect();
    documents.join("\n").into_bytes()
}

/// The error-output document of one record: how the delivery failed, when it was attempted
/// and the record's own data, under the member names Firehose's error output carries
/// (`errorCode`, `errorMessage`, `attemptsMade`, `arrivalTimestamp`,
/// `attemptEndingTimestamp`, `rawData`).
///
/// `rawData` is the record's data base64-encoded, exactly as the put carried it on the
/// wire, so nothing of the record is lost by the failure. The two timestamps are epoch
/// MILLISECONDS: `arrivalTimestamp` is when the record was ingested and
/// `attemptEndingTimestamp` when this delivery attempt ended.
fn error_envelope(
    record: &StoredRecord,
    failure: DeliveryFailure,
    now: SystemTime,
) -> ::serde_json::Value {
    ::serde_json::json!({
        "attemptsMade": 1,
        "arrivalTimestamp": epoch_millis(record.received_at),
        "errorCode": failure.error_code,
        "errorMessage": failure.error_message,
        "attemptEndingTimestamp": epoch_millis(now),
        "rawData": ::aws_smithy_types::base64::encode(&record.data),
    })
}

/// `at` as epoch milliseconds, which is how the error-output document carries an instant.
fn epoch_millis(at: SystemTime) -> u64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_millis())
        .unwrap_or_default()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// `body` encoded as the destination's `CompressionFormat` asks, with the file extension
/// the delivered object's name carries.
///
/// `None` when the encoder failed, in which case nothing is delivered for this object
/// rather than a body in a format the destination did not ask for.
fn compressed(body: Vec<u8>, target: &S3Target) -> Option<(Vec<u8>, String)> {
    const UNCOMPRESSED: &str = "UNCOMPRESSED";
    let format = target
        .compression_format
        .as_deref()
        .unwrap_or(UNCOMPRESSED);
    let (encoded, default_extension) = match format {
        // "Specify a file extension. It will override the default file extension"
        // (`FileExtension`) — the default being the one the compression format carries:
        // Firehose appends `.gz` to a GZIP object and `.zip` to a ZIP one, and leaves an
        // uncompressed or Snappy object without one.
        "GZIP" => (gzip(&body)?, ".gz"),
        "ZIP" => (zip(&body)?, ".zip"),
        "Snappy" => (snappy(&body)?, ""),
        "HADOOP_SNAPPY" => (hadoop_snappy(&body)?, ""),
        UNCOMPRESSED => (body, ""),
        other => {
            ::tracing::warn!(%other, "the destination asks for a compression format this emulator cannot write");
            return None;
        }
    };
    let extension = target
        .file_extension
        .clone()
        .filter(|extension| !extension.is_empty())
        .unwrap_or_else(|| default_extension.to_string());
    Some((encoded, extension))
}

/// `body` as a gzip stream.
fn gzip(body: &[u8]) -> Option<Vec<u8>> {
    let mut encoder =
        ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
    encoder.write_all(body).ok()?;
    encoder.finish().ok()
}

/// `body` as a zip archive holding it as its single entry.
fn zip(body: &[u8]) -> Option<Vec<u8>> {
    /// The name of the archive's single entry. A ZIP object a Firehose stream delivers is
    /// one buffer's data in one archive, so one entry is what it holds.
    const ENTRY_NAME: &str = "data";
    let mut archive = ::zip::ZipWriter::new(::std::io::Cursor::new(Vec::new()));
    archive
        .start_file(ENTRY_NAME, ::zip::write::SimpleFileOptions::default())
        .ok()?;
    archive.write_all(body).ok()?;
    Some(archive.finish().ok()?.into_inner())
}

/// `body` in the Snappy framing format.
fn snappy(body: &[u8]) -> Option<Vec<u8>> {
    let mut encoder = ::snap::write::FrameEncoder::new(Vec::new());
    encoder.write_all(body).ok()?;
    encoder.into_inner().ok()
}

/// `body` in the Hadoop Snappy container: the uncompressed length of the stream, then one
/// block as its compressed length followed by raw Snappy, each length a 32-bit big-endian
/// count. That framing — and not the Snappy one above — is what distinguishes
/// `HADOOP_SNAPPY` from `Snappy`.
fn hadoop_snappy(body: &[u8]) -> Option<Vec<u8>> {
    let compressed = ::snap::raw::Encoder::new().compress_vec(body).ok()?;
    let mut framed = Vec::with_capacity(compressed.len() + 8);
    framed.extend_from_slice(&u32::try_from(body.len()).ok()?.to_be_bytes());
    framed.extend_from_slice(&u32::try_from(compressed.len()).ok()?.to_be_bytes());
    framed.extend_from_slice(&compressed);
    Some(framed)
}

/// The name a delivered object carries, under whatever prefix it is written:
/// `<stream>-<version>-<YYYY-MM-DD-HH-MM-SS>-<random>`, the form Firehose names an object
/// it delivers to Amazon S3 with. `ordinal` distinguishes two objects of one flush written
/// in the same second.
fn object_name(delivery: &PendingDelivery, now: SystemTime, ordinal: usize) -> String {
    let stamp = utc_fields(now).map_or_else(String::new, |fields| {
        format!(
            "{}-{}-{}-{}-{}-{}",
            fields.year, fields.month, fields.day, fields.hour, fields.minute, fields.second
        )
    });
    let random = ::uuid::Uuid::new_v4();
    format!(
        "{}-{}-{stamp}-{ordinal}-{random}",
        delivery.stream, delivery.version_id
    )
}

/// The UTC calendar fields of an instant, each zero-padded as a key carries it.
struct UtcFields {
    year: String,
    month: String,
    day: String,
    hour: String,
    minute: String,
    second: String,
}

/// The UTC calendar fields of `at`, or `None` for an instant the library cannot render.
///
/// The calendar conversion is the LIBRARY's: `DateTime::fmt(Format::DateTime)` renders an
/// instant as RFC 3339 in UTC (`YYYY-MM-DDThh:mm:ss[.fff]Z`), and the fields are read off
/// that text rather than computed here.
fn utc_fields(at: SystemTime) -> Option<UtcFields> {
    let rendered = ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()?;
    let (date, time) = rendered.split_once('T')?;
    let mut date = date.split('-');
    let mut time = time.split(':');
    let fields = UtcFields {
        year: date.next()?.to_string(),
        month: date.next()?.to_string(),
        day: date.next()?.to_string(),
        hour: time.next()?.to_string(),
        minute: time.next()?.to_string(),
        // The seconds field carries the zone (and any fractional part) with it.
        second: time
            .next()?
            .split(['.', 'Z', 'z', '+'])
            .next()?
            .to_string(),
    };
    Some(fields)
}

/// The prefix a delivered object is written under: `prefix` with its `!{...}` namespaces
/// expanded, plus the automatic time prefix when it carries none.
///
/// The namespaces a prefix may carry, with what each expands to here:
///
/// * `partitionKeyFromQuery:<name>` — the value `keys` holds for `<name>`; a name the
///   query derived no value for makes the whole prefix unresolvable, which is a record
///   whose metadata extraction failed.
/// * `timestamp:<pattern>` — the delivery's own instant under `<pattern>`
///   ([`format_timestamp`]).
/// * `firehose:error-output-type` — why the record is in the error output.
/// * `firehose:random-string` — a value unique to this object.
/// * `partitionKeyFromLambda:<name>` — the keys a Lambda data transformation returned.
///   This emulator runs no transformation, so there are none and the record's partition
///   keys could not be determined, rather than it landing under a prefix with a value
///   invented for it.
///
/// "The \"YYYY/MM/DD/HH\" time format prefix is automatically used for delivered Amazon S3
/// files" (`Prefix`), so a prefix carrying no expression at all gets it appended; one that
/// carries an expression says where its objects go itself.
///
/// `Err` carries how the delivery of the records this prefix could not be resolved for
/// failed.
fn expand_prefix(
    prefix: &str,
    keys: &BTreeMap<String, String>,
    error_output_type: Option<&str>,
    now: SystemTime,
) -> Result<String, DeliveryFailure> {
    const OPEN: &str = "!{";
    let mut expanded = String::new();
    let mut rest = prefix;
    let mut expressions = 0usize;
    while let Some(at) = rest.find(OPEN) {
        let (literal, from_open) = rest.split_at(at);
        expanded.push_str(literal);
        // Everything up to the `!{` is now in `expanded`, so what is still to be read is
        // the text FROM it: a break below leaves exactly that to be appended as written.
        rest = from_open;
        let Some(inside) = from_open.strip_prefix(OPEN) else {
            break;
        };
        let Some(end) = inside.find('}') else {
            // An unclosed expression is not an expression: the text stands as written.
            break;
        };
        let (expression, after) = inside.split_at(end);
        let Some(after) = after.strip_prefix('}') else {
            break;
        };
        rest = after;
        expressions += 1;
        let (namespace, argument) = expression
            .split_once(':')
            .unwrap_or((expression.trim(), ""));
        match (namespace.trim(), argument.trim()) {
            ("partitionKeyFromQuery", name) => {
                expanded.push_str(keys.get(name).ok_or(METADATA_EXTRACTION_FAILED)?);
            }
            ("partitionKeyFromLambda", _) => return Err(METADATA_EXTRACTION_FAILED),
            ("timestamp", pattern) => {
                expanded
                    .push_str(&format_timestamp(now, pattern).ok_or(METADATA_EXTRACTION_FAILED)?);
            }
            ("firehose", "error-output-type") if error_output_type.is_some() => {
                expanded.push_str(error_output_type.unwrap_or_default());
            }
            ("firehose", "random-string") => {
                expanded.push_str(&::uuid::Uuid::new_v4().simple().to_string());
            }
            // A namespace this emulator does not know — and the error output type, in a
            // prefix that is not an error output prefix — is left as the configuration
            // wrote it, so the prefix stays the caller's text rather than silently losing
            // part of it.
            _ => {
                expanded.push_str(OPEN);
                expanded.push_str(expression);
                expanded.push('}');
                expressions -= 1;
            }
        }
    }
    expanded.push_str(rest);
    if expressions == 0 {
        let fields = utc_fields(now).ok_or(METADATA_EXTRACTION_FAILED)?;
        expanded.push_str(&format!(
            "{}/{}/{}/{}/",
            fields.year, fields.month, fields.day, fields.hour
        ));
    }
    Ok(expanded)
}

/// `at` under a `!{timestamp:...}` pattern.
///
/// The pattern is a Java `DateTimeFormatter` one, of which the fields a delivery prefix is
/// written from are read: `y` (year), `M` (month), `d` (day), `H` (hour of day), `m`
/// (minute) and `s` (second), each repeated as many times as the field is wide, with
/// everything else — separators, literal text — carried through as written. A run of any
/// other pattern letter is carried through as written too, so a prefix this emulator
/// cannot read stays the caller's text rather than becoming a value guessed for it.
fn format_timestamp(at: SystemTime, pattern: &str) -> Option<String> {
    let fields = utc_fields(at)?;
    let mut formatted = String::new();
    let mut letters = pattern.chars().peekable();
    while let Some(letter) = letters.next() {
        if !letter.is_ascii_alphabetic() {
            formatted.push(letter);
            continue;
        }
        let mut width = 1usize;
        while letters.peek() == Some(&letter) {
            letters.next();
            width += 1;
        }
        let field = match letter {
            'y' | 'u' => Some(&fields.year),
            'M' => Some(&fields.month),
            'd' => Some(&fields.day),
            'H' => Some(&fields.hour),
            'm' => Some(&fields.minute),
            's' => Some(&fields.second),
            _ => None,
        };
        match field {
            // A field is padded to the pattern's width and, where the pattern is narrower
            // than the field, carries its least significant digits — `yy` is a two-digit
            // year.
            Some(value) => formatted.push_str(&fit(value, width)),
            None => formatted.extend(::std::iter::repeat_n(letter, width)),
        }
    }
    Some(formatted)
}

/// `value` as exactly `width` characters: zero-padded on the left when it is shorter, and
/// its last `width` characters when it is longer.
fn fit(value: &str, width: usize) -> String {
    let held = value.chars().count();
    if held >= width {
        return value.chars().skip(held - width).collect();
    }
    let mut fitted = "0".repeat(width - held);
    fitted.push_str(value);
    fitted
}

/// Start the one in-process task that waits for this store's earliest due delivery,
/// delivers what is then due, and stands down once nothing is owed.
///
/// A producer that puts a record makes no further Firehose call, so a delivery performed
/// only by the next call would never be performed at all — this is what makes the data
/// plane observable without one. It does not change WHEN a delivery happens: the task only
/// calls [`deliver_due`], which applies what the stored due instant has already made due,
/// so a caller reading the destination before that instant still sees the data buffered.
/// The task waits for the clock so that no request ever has to.
///
/// At most one pump runs per store (the store carries the claim), it is bounded by
/// [`MAX_DELIVERY_WAKE_UPS`] wake-ups, and with no async runtime to spawn onto there is no
/// pump — the deliveries are then made by the next operation instead.
pub fn drive_delivery_clock<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = start_pump(&mut state.lock()) else {
        return;
    };
    let engine = Arc::clone(engine);
    let pumped = ::engine::ServiceState::clone(&state);
    let handle = runtime.spawn(async move {
        // `true` only if the wake-up cap is what ended the loop; a pump that ran out of
        // work has already released its claim inside the one critical section that
        // observed it.
        let mut capped = true;
        for _ in 0..MAX_DELIVERY_WAKE_UPS {
            let now = SystemTime::now();
            let Some(due) = next_due_or_release_pump(&mut state.lock(), generation) else {
                capped = false;
                break;
            };
            // Every wake-up waits, even for an instant already past: that is what keeps a
            // due buffer from costing a CPU-bound spin, and it hands the runtime back
            // between wake-ups.
            let wait = due
                .duration_since(now)
                .unwrap_or_default()
                .clamp(MIN_DELIVERY_WAIT, MAX_DELIVERY_WAIT);
            ::tokio::time::sleep(wait).await;
            deliver_due(&*engine, SystemTime::now(), Extractions::Run).await;
        }
        if capped {
            // Only THIS pump's claim, so a pump started since is left holding its own.
            release_pump(&mut state.lock(), generation);
        }
    });
    // The handle is kept so the pump it replaces is aborted rather than merely forgotten.
    set_pump(&mut pumped.lock(), generation, handle);
}

/// Claim the single delivery pump for this store: the claim's GENERATION when the caller
/// may start one, `None` when one already runs.
fn start_pump(store: &mut ComAmazonawsFirehoseFirehose20150804State) -> Option<u64> {
    if store.delivery_pump_running {
        return None;
    }
    store.delivery_pump_running = true;
    store.delivery_pump_generation = store.delivery_pump_generation.wrapping_add(1);
    Some(store.delivery_pump_generation)
}

/// Release the claim `generation` took — and ONLY that one, so a pump that ended on the
/// wake-up cap cannot clear a claim a pump started since is holding.
fn release_pump(store: &mut ComAmazonawsFirehoseFirehose20150804State, generation: u64) {
    if store.delivery_pump_generation == generation {
        store.delivery_pump_running = false;
    }
}

/// Keep the running pump's task handle, aborting the one it replaces.
///
/// Only when `generation` is still the CURRENT claim. A pump can end before its starter
/// gets here (it releases its own claim as soon as the store owes nothing), and a put that
/// claimed the pump since then has stored a handle of its own: replacing that one would
/// abort a live pump and leave its claim held for ever. So a stale starter installs
/// nothing and aborts nothing — its own task has already released its claim, so it stands
/// down on its own.
fn set_pump(
    store: &mut ComAmazonawsFirehoseFirehose20150804State,
    generation: u64,
    handle: ::tokio::task::JoinHandle<()>,
) {
    if store.delivery_pump_generation != generation {
        return;
    }
    if let Some(previous) = store.delivery_pump.replace(handle) {
        previous.abort();
    }
}

/// What the delivery pump waits for next, or `None` — having RELEASED its claim — when the
/// store owes nothing.
///
/// Observing "nothing is owed" and standing down happen in ONE critical section, which is
/// what makes the hand-over race-free: a put holds this same lock to buffer its record
/// BEFORE it tries to claim the pump, so either this call sees that buffer and the running
/// pump keeps it, or the put's claim finds the flag cleared and starts a pump of its own.
fn next_due_or_release_pump(
    store: &mut ComAmazonawsFirehoseFirehose20150804State,
    generation: u64,
) -> Option<SystemTime> {
    match next_due_at(store) {
        Some(due) => Some(due),
        None => {
            release_pump(store, generation);
            None
        }
    }
}
