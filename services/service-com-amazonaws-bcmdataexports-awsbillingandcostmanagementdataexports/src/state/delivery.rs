// SPDX-License-Identifier: CC0-1.0

//! The DELIVERY the service owes a data export: the file it writes into the destination
//! bucket, and the statuses the execution recording that delivery moves through.
//!
//! "The cadence for Amazon Web Services to update the data export in your S3 bucket"
//! (smithy_query shape `RefreshCadence`) is work the SERVICE does, so every definition it
//! persists is owed a delivery, and a delivery RUNS the definition's `DataQuery` and writes
//! the result into the destination the definition names — "the destination Amazon Simple
//! Storage Service (Amazon S3) bucket name and object keys of a data exports file"
//! (smithy_query shape `S3Destination`).
//!
//! That delivery is PERFORMED here, against the object store of the Amazon S3 service
//! mounted on this same engine: the object it writes is one that service's own
//! `ListObjectsV2` and `GetObject` then serve, and the execution reports the outcome of the
//! write that really happened — the modelled `DELIVERY_SUCCESS` once the object is stored,
//! and the modelled `DELIVERY_FAILURE` with the reason the destination refused it. Because
//! an export file was then generated, the export's `ExportStatus.LastRefreshedAt` ("The
//! timestamp of when the export was last generated") moves to the instant the delivery
//! finished; a delivery that wrote nothing moves nothing.
//!
//! WHAT THE FILE HOLDS: the columns the definition's statement projects, read off the
//! parsed statement against the published table catalog
//! ([`super::query::delivered_columns`]), and the rows that statement selects from this
//! account's cost and usage data — of which this emulator holds none, so the file carries
//! its header row and no data row, which is what an export covering no usage delivers.
//! Nothing is claimed beyond that: the file is encoded in the format and compression the
//! definition states, by the maintained implementation of each of them, so a client reading
//! the object back decodes a real GZIP stream, ZIP archive or Parquet file.
//!
//! WHEN it happens is the CLOCK's, not a caller's: the instants are stored by the call that
//! owed the delivery ([`super::PendingDelivery`], [`super::StoredExecution`]) and
//! [`materialize_due_deliveries`] applies whatever those stored instants have already made
//! due. Nothing sleeps for one, and a pass that runs late writes exactly what a pass that
//! ran on time would have written, because every value it writes is derived from the stored
//! instants rather than from the clock it read.

use ::std::io::Write;
use ::std::sync::Arc;
use ::std::time::{Duration, SystemTime};

use crate::ops::types::{Export, S3OutputConfigurations};

use super::{
    service_state, timestamp, ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState,
    EXECUTION_STATUS_INITIATION_IN_PROCESS, EXECUTION_STATUS_QUERY_FAILURE,
    EXECUTION_STATUS_REASON_INTERNAL_FAILURE,
};

/// The `ExecutionStatusCode` of a delivery whose query is waiting to run: the step after the
/// delivery is initiated. One of the modelled values (see
/// `crate::ops::types::EXECUTION_STATUS_CODE_VALUES`).
pub const EXECUTION_STATUS_QUERY_QUEUED: &str = "QUERY_QUEUED";

/// The `ExecutionStatusCode` of a delivery whose query is running.
pub const EXECUTION_STATUS_QUERY_IN_PROCESS: &str = "QUERY_IN_PROCESS";

/// The `ExecutionStatusCode` of a delivery whose file is being written to the destination.
pub const EXECUTION_STATUS_DELIVERY_IN_PROCESS: &str = "DELIVERY_IN_PROCESS";

/// The terminal `ExecutionStatusCode` of a delivery whose file IS in the destination
/// bucket.
pub const EXECUTION_STATUS_DELIVERY_SUCCESS: &str = "DELIVERY_SUCCESS";

/// The terminal `ExecutionStatusCode` of a delivery whose query ran and whose file the
/// destination did not take.
pub const EXECUTION_STATUS_DELIVERY_FAILURE: &str = "DELIVERY_FAILURE";

/// The `ExecutionStatusReason` of a delivery the destination would not let this service
/// write: one of the modelled values (smithy_query shape `ExecutionStatusReason`), and the
/// one that says the permission to write was what was missing.
pub const EXECUTION_STATUS_REASON_INSUFFICIENT_PERMISSION: &str = "INSUFFICIENT_PERMISSION";

/// The statuses one delivery moves through before it reaches a terminal one, in order: the
/// work the service does for it — initiating the delivery, queueing its query, running that
/// query, then writing the file — each a modelled `ExecutionStatusCode`.
///
/// A delivery is in the FIRST of them from the instant it is initiated, and in each later
/// one for an equal share of the span to its terminal status, so a client polling an
/// execution observes the pipeline it is really in rather than one status standing for all
/// of it. Which one is in force at an instant is a function of the two stored instants
/// alone ([`pipeline_status`]).
pub const DELIVERY_PIPELINE: &[&str] = &[
    EXECUTION_STATUS_INITIATION_IN_PROCESS,
    EXECUTION_STATUS_QUERY_QUEUED,
    EXECUTION_STATUS_QUERY_IN_PROCESS,
    EXECUTION_STATUS_DELIVERY_IN_PROCESS,
];

/// The non-terminal status of a delivery initiated at `initiated_at` and due to finish at
/// `complete_at`, as of `now`, with the instant that status fell due.
///
/// `None` for a delivery that finishes before it starts, which no schedule of this service
/// produces. A span too short to divide leaves the delivery in the status it was initiated
/// in: there is no room in it for a later step to fall due.
pub fn pipeline_status(
    initiated_at: SystemTime,
    complete_at: SystemTime,
    now: SystemTime,
) -> Option<(&'static str, SystemTime)> {
    let span = complete_at.duration_since(initiated_at).ok()?;
    let steps = u32::try_from(DELIVERY_PIPELINE.len()).ok()?;
    let step = span.checked_div(steps)?;
    let mut in_force = (EXECUTION_STATUS_INITIATION_IN_PROCESS, initiated_at);
    if step.is_zero() {
        return Some(in_force);
    }
    for (index, code) in DELIVERY_PIPELINE.iter().enumerate().skip(1) {
        let due_at = initiated_at + step * u32::try_from(index).ok()?;
        if due_at > now {
            break;
        }
        in_force = (code, due_at);
    }
    Some(in_force)
}

/// One delivery whose terminal status the clock has made due: everything performing it
/// needs, copied out of this service's store so the object store can be locked WITHOUT this
/// service's own lock held.
#[derive(Debug, Clone)]
pub struct DueDelivery {
    /// The export this delivery is for, which is also what the destination's grant is
    /// conditioned on.
    pub export_arn: String,
    /// The execution recording it.
    pub execution_id: String,
    /// The STORED instant this delivery finishes at: the instant its file is dated from and
    /// its terminal status is reported at, whenever this pass happens to run.
    pub due_at: SystemTime,
    /// The definition this delivery runs — the snapshot the execution was generated from.
    pub export: Export,
}

/// What performing one delivery did: the terminal `ExecutionStatusCode` to record, the
/// modelled `ExecutionStatusReason` a failure reports, and the key a delivery that was
/// written landed at.
#[derive(Debug, Clone)]
pub struct DeliveryOutcome {
    /// The terminal status this delivery reached.
    pub status_code: &'static str,
    /// Why it failed, for a delivery that did not succeed; `None` for one that did, which
    /// has nothing to report a reason for.
    pub status_reason: Option<&'static str>,
    /// The object key the export file was written at, for a delivery that wrote one.
    pub key: Option<String>,
}

impl DeliveryOutcome {
    /// The outcome of a delivery whose file IS in the destination bucket, at `key`.
    fn delivered(key: String) -> Self {
        Self {
            status_code: EXECUTION_STATUS_DELIVERY_SUCCESS,
            status_reason: None,
            key: Some(key),
        }
    }

    /// The outcome of a delivery that did not happen: `status_code` is the step it failed
    /// at and `status_reason` the modelled reason it failed for.
    fn failed(status_code: &'static str, status_reason: &'static str) -> Self {
        Self {
            status_code,
            status_reason: Some(status_reason),
            key: None,
        }
    }
}

/// How many deliveries ONE pass performs.
///
/// The bound on the work a single request does inline: the store can owe a delivery per
/// export and per update of one, so without this a burst of calls inside one schedule window
/// would leave a later request running every one of those queries and writing every one of
/// those files before it answers. With it, one pass costs at most this many bounded parses
/// and files (each of at most [`super::MAX_DELIVERED_COLUMNS`] columns), and what it leaves
/// behind is still due for the next pass — see
/// [`ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState::deliveries_due`].
const MAX_DELIVERIES_PER_PASS: usize = 16;

/// Apply everything the CLOCK already owes this service's deliveries, as of `now`, and
/// PERFORM the ones that have fallen due.
///
/// Every handler of this service calls this before it answers, so no caller has to poll a
/// particular operation for a delivery to happen. Three steps, each with at most one store
/// locked:
///
/// 1. this service's store applies what the clock owes its schedule
///    ([`ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState::materialize_due`])
///    and reports the deliveries whose terminal status is due;
/// 2. each of those is performed with this service's lock RELEASED — the query is read, the
///    file is encoded, and the object store alone is locked to write it;
/// 3. this service's store records what each delivery did.
///
/// The two stores are never locked at once, in either order, so no pair of services on this
/// engine can deadlock on them. Performing a delivery twice writes the same object at the
/// same key and records the same terminal status, so two passes racing on one due delivery
/// leave exactly what one pass would have.
///
/// WHAT ONE PASS COSTS is bounded on both factors: at most [`MAX_DELIVERIES_PER_PASS`]
/// deliveries, each one bounded parse of its stored statement and one file of at most
/// [`super::MAX_DELIVERED_COLUMNS`] columns. A delivery this pass leaves behind is still due
/// for the next one.
pub fn materialize_due_deliveries<Eng: Send + Sync + 'static>(engine: &Eng, now: SystemTime) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let due: Vec<DueDelivery> = {
        let mut store = state.lock();
        store.materialize_due(now);
        store.deliveries_due(now)
    };
    if due.is_empty() {
        return;
    }
    let performed: Vec<(DueDelivery, DeliveryOutcome)> = due
        .into_iter()
        .map(|delivery| {
            let outcome = perform_delivery(engine, &delivery);
            (delivery, outcome)
        })
        .collect();
    let mut store = state.lock();
    for (delivery, outcome) in &performed {
        store.record_delivery(delivery, outcome);
    }
}

/// How many delivery-clock tasks this process runs at once.
///
/// One request owes at most one delivery, so this is what bounds the tasks a caller making
/// request after request can have in flight — the work each one does is bounded, but their
/// NUMBER would otherwise follow the request rate. Past this many, no task is started and
/// the delivery is applied by the next request to reach the store, which is the mechanism
/// that would have applied it anyway.
const MAX_DELIVERY_CLOCK_TASKS: usize = 64;

/// How many of those tasks are in flight now.
static DELIVERY_CLOCK_TASKS: ::std::sync::atomic::AtomicUsize =
    ::std::sync::atomic::AtomicUsize::new(0);

/// One task's place in [`MAX_DELIVERY_CLOCK_TASKS`], released when it is dropped — which a
/// task that finished, panicked or was cancelled with its runtime all do, so a place cannot
/// be lost and the bound cannot drift closed.
struct DeliveryClockPermit;

impl DeliveryClockPermit {
    /// A place for one more task, or `None` when this process already runs its maximum.
    fn take() -> Option<Self> {
        use ::std::sync::atomic::Ordering;
        DELIVERY_CLOCK_TASKS
            .try_update(Ordering::AcqRel, Ordering::Acquire, |running| {
                (running < MAX_DELIVERY_CLOCK_TASKS).then_some(running + 1)
            })
            .ok()
            .map(|_| Self)
    }
}

impl Drop for DeliveryClockPermit {
    fn drop(&mut self) {
        DELIVERY_CLOCK_TASKS.fetch_sub(1, ::std::sync::atomic::Ordering::AcqRel);
    }
}

/// Start the one short in-process task that applies what the clock owes this service's
/// deliveries once `due_in` has passed.
///
/// A client that creates an export and then reads the destination bucket makes no further
/// call to THIS service, so a delivery performed only by the next such call could be a
/// delivery never performed — and the delivery is the service's own work, owed by the
/// cadence rather than by anything the caller does. The task does not change WHEN a delivery
/// happens: it only calls [`materialize_due_deliveries`], which applies what the stored
/// instants have already made due, so a client reading the destination before that instant
/// still finds nothing and a delivery materialised earlier by another request is not
/// performed twice. With no async runtime to spawn onto there is no task, and the next
/// request applies it instead — as it also does past [`MAX_DELIVERY_CLOCK_TASKS`].
pub fn drive_delivery_clock<Eng: Send + Sync + 'static>(engine: &Arc<Eng>, due_in: Duration) {
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    // The place is taken BEFORE the task is spawned, so the count bounds what is in flight
    // rather than trailing it, and the task holds it until it is dropped.
    let Some(permit) = DeliveryClockPermit::take() else {
        return;
    };
    let engine = Arc::clone(engine);
    runtime.spawn(async move {
        ::tokio::time::sleep(due_in).await;
        materialize_due_deliveries(&*engine, SystemTime::now());
        drop(permit);
    });
}

impl ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState {
    /// The deliveries whose terminal status `now` has made due: the ones a pass has to
    /// perform, OLDEST FIRST and at most [`MAX_DELIVERIES_PER_PASS`] of them.
    ///
    /// The cap is what bounds the work ONE pass does: the store can hold a delivery per
    /// export and per update of it, so a burst of calls inside one schedule window would
    /// otherwise leave a single later request performing every one of them inline. Oldest
    /// first — and by execution id where two fell due at the same instant, so the order is
    /// the same on every pass — so nothing is starved: what this pass leaves behind is still
    /// due, and the next request (or the clock task) performs it.
    ///
    /// Nothing is taken off the store here — the due instant stays on the execution until
    /// the outcome is recorded against it ([`Self::record_delivery`]) — so a pass that is
    /// interrupted between performing and recording leaves a delivery that is still owed
    /// rather than one nothing will ever close.
    fn deliveries_due(&self, now: SystemTime) -> Vec<DueDelivery> {
        let mut due: Vec<DueDelivery> = self
            .executions
            .values()
            .filter_map(|execution| {
                let due_at = execution
                    .completion_due_at
                    .filter(|due_at| *due_at <= now)?;
                Some(DueDelivery {
                    export_arn: execution.export_arn.clone(),
                    execution_id: execution.execution_id.clone(),
                    due_at,
                    export: execution.export.clone(),
                })
            })
            .collect();
        due.sort_by(|left, right| {
            (left.due_at, &left.execution_id).cmp(&(right.due_at, &right.execution_id))
        });
        due.truncate(MAX_DELIVERIES_PER_PASS);
        due
    }

    /// Record what `delivery` did: its execution's terminal status, and — for a delivery
    /// that wrote the export file — the instant the export was last generated at.
    ///
    /// The instants come from the STORED due instant rather than from the clock this pass
    /// read, so a pass that runs late records what one that ran on time would have. An
    /// execution that no longer owes its terminal status at exactly that instant is left
    /// alone: it was closed by another pass, or its delivery was rescheduled, and in neither
    /// case does an outcome computed for the old instant belong to it.
    fn record_delivery(&mut self, delivery: &DueDelivery, outcome: &DeliveryOutcome) {
        let key = (delivery.export_arn.clone(), delivery.execution_id.clone());
        let Some(execution) = self.executions.get_mut(&key) else {
            return;
        };
        if execution.completion_due_at != Some(delivery.due_at) {
            return;
        }
        let completed_at = timestamp(delivery.due_at);
        execution.completion_due_at = None;
        execution.status.status_code = Some(outcome.status_code.to_string());
        execution.status.status_reason = outcome.status_reason.map(str::to_string);
        execution.status.completed_at = Some(completed_at);
        execution.status.last_updated_at = Some(completed_at);
        if outcome.status_code != EXECUTION_STATUS_DELIVERY_SUCCESS {
            return;
        }
        // "The timestamp of when the export was last generated" (smithy_query shape
        // `ExportStatus`, member `LastRefreshedAt`): a file WAS generated, at the instant
        // this delivery finished. It only ever moves forward, so a delivery recorded after a
        // later one does not take the export back to an earlier generation; the comparison
        // is over the whole instant, not its seconds alone.
        if let Some(export) = self.exports.get_mut(&delivery.export_arn) {
            let generated_earlier = export.status.last_refreshed_at.is_none_or(|refreshed| {
                (refreshed.secs(), refreshed.subsec_nanos())
                    <= (completed_at.secs(), completed_at.subsec_nanos())
            });
            if generated_earlier {
                export.status.last_refreshed_at = Some(completed_at);
            }
        }
    }
}

/// Perform ONE delivery: run its query, encode the export file, and write that file into
/// the destination bucket.
///
/// Neither this service's store nor the object store is locked when the file is built, and
/// only the object store is locked while it is written.
fn perform_delivery<Eng: Send + Sync + 'static>(
    engine: &Eng,
    delivery: &DueDelivery,
) -> DeliveryOutcome {
    let destination = &delivery.export.destination_configurations;
    let s3 = &destination.s3_destination;

    // THE QUERY. Its columns are read off the parsed statement against the published
    // catalog; a statement this read refuses selects no rows, which is the step before
    // anything reaches the destination.
    let columns = match super::delivered_columns(&delivery.export.data_query) {
        Ok(columns) => columns,
        Err(_) => {
            ::tracing::warn!(
                export_arn = %delivery.export_arn,
                "the data query of a due delivery could not be read, so its query failed"
            );
            return DeliveryOutcome::failed(
                EXECUTION_STATUS_QUERY_FAILURE,
                EXECUTION_STATUS_REASON_INTERNAL_FAILURE,
            );
        }
    };

    // THE FILE, in the encoding the definition states.
    let file = match export_file(&delivery.export.name, &columns, &s3.s3_output_configurations) {
        Ok(file) => file,
        Err(reason) => {
            ::tracing::warn!(
                export_arn = %delivery.export_arn,
                %reason,
                "the export file of a due delivery could not be written"
            );
            return DeliveryOutcome::failed(
                EXECUTION_STATUS_DELIVERY_FAILURE,
                EXECUTION_STATUS_REASON_INTERNAL_FAILURE,
            );
        }
    };

    // THE PERMISSION, read again at delivery: the grant that lets this service write lives
    // on the destination bucket's policy, and the caller may have taken it off since the
    // definition was accepted — which is a delivery the destination does not permit, not a
    // fault of this service.
    if super::validate_delivery_grant(engine, destination, &delivery.export_arn).is_err() {
        return DeliveryOutcome::failed(
            EXECUTION_STATUS_DELIVERY_FAILURE,
            EXECUTION_STATUS_REASON_INSUFFICIENT_PERMISSION,
        );
    }

    let key = delivery_key(delivery, &file);
    let Some(objects) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        ::tracing::warn!("no object store is reachable from this engine, so a delivery wrote no export file");
        return DeliveryOutcome::failed(
            EXECUTION_STATUS_DELIVERY_FAILURE,
            EXECUTION_STATUS_REASON_INTERNAL_FAILURE,
        );
    };
    let written = {
        let mut store = objects.lock();
        store.put_object(
            &s3.s3_bucket,
            &key,
            ::service_com_amazonaws_s3_amazons3::state::Object::stored(
                file.body,
                Some(file.content_type.to_string()),
            ),
        )
    };
    match written {
        Some(
            ::service_com_amazonaws_s3_amazons3::state::ObjectWrite::Created
            | ::service_com_amazonaws_s3_amazons3::state::ObjectWrite::Replaced,
        ) => DeliveryOutcome::delivered(key),
        // The destination holds no such bucket, so there is nothing this service may write
        // into: the file was not delivered for want of a destination that admits it, which
        // is what the modelled reason above reports.
        None => DeliveryOutcome::failed(
            EXECUTION_STATUS_DELIVERY_FAILURE,
            EXECUTION_STATUS_REASON_INSUFFICIENT_PERMISSION,
        ),
        // The object store refused the write for a condition of its own (a key or a budget
        // of the destination): the delivery failed on this side of the write.
        Some(refusal) => {
            ::tracing::warn!(
                bucket = %s3.s3_bucket,
                ?refusal,
                "the destination bucket did not take an export file"
            );
            DeliveryOutcome::failed(
                EXECUTION_STATUS_DELIVERY_FAILURE,
                EXECUTION_STATUS_REASON_INTERNAL_FAILURE,
            )
        }
    }
}

/// The part number one delivery's single file carries: an export is delivered as one file
/// per delivery here, which is its first part.
const REPORT_PART: &str = "00001";

/// The segment of a delivery's path holding the exported DATA.
const DATA_SEGMENT: &str = "data";

/// The partition of that path naming the billing period the data covers.
const BILLING_PERIOD_PARTITION: &str = "BILLING_PERIOD";

/// The `OverwriteOption` under which each delivery REPLACES the file the previous one wrote
/// (smithy_query shape `OverwriteOption`: `CREATE_NEW_REPORT`, `OVERWRITE_REPORT`).
pub const OVERWRITE_REPORT: &str = "OVERWRITE_REPORT";

/// The key `delivery` writes its file at:
/// `<S3Prefix>/<Name>/data/BILLING_PERIOD=<YYYY-MM>/<Name>[-<delivery>]-00001.<extension>`.
///
/// The leading path is the model's: `S3Prefix` is "The S3 path prefix you want prepended to
/// the name of your data export" (smithy_query shape `S3Destination`, member `S3Prefix`), so
/// the prefix comes first and the export's own name under it, with the data of one billing
/// period in its own partition below.
///
/// The `Overwrite` of the definition decides the FILE NAME, which is what makes the two
/// modelled dispositions observable: "You have the choice to overwrite the previous version
/// or to be delivered in addition to the previous versions" (smithy_query shape
/// `S3OutputConfigurations`, member `Overwrite`). `OVERWRITE_REPORT` therefore names the
/// same file on every delivery of one export in one billing period, so a delivery replaces
/// the previous file, while `CREATE_NEW_REPORT` carries the delivery's own id and so is
/// delivered in addition to the files already there.
fn delivery_key(delivery: &DueDelivery, file: &ExportFile) -> String {
    let s3 = &delivery.export.destination_configurations.s3_destination;
    let name = &delivery.export.name;
    let report = if s3.s3_output_configurations.overwrite == OVERWRITE_REPORT {
        format!("{name}-{REPORT_PART}.{}", file.extension)
    } else {
        format!(
            "{name}-{}-{REPORT_PART}.{}",
            delivery.execution_id, file.extension
        )
    };
    let path = format!(
        "{name}/{DATA_SEGMENT}/{BILLING_PERIOD_PARTITION}={}/{report}",
        billing_period(delivery.due_at)
    );
    // A prefix is required but may be empty, and the separators of the path are this
    // service's: a prefix written with leading or trailing slashes names the same place as
    // one without them.
    match s3.s3_prefix.trim_matches('/') {
        "" => path,
        prefix => format!("{prefix}/{path}"),
    }
}

/// The billing period the file one delivery writes covers, as the `YYYY-MM` its partition is
/// named by: the month the delivery ran in, UTC — the zone every instant this service
/// reports is in.
fn billing_period(at: SystemTime) -> String {
    /// How many characters of an RFC 3339 instant spell its year and month.
    const YEAR_AND_MONTH: usize = "YYYY-MM".len();
    timestamp(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .and_then(|rendered| rendered.get(..YEAR_AND_MONTH).map(str::to_string))
        .unwrap_or_default()
}

/// One export file: the bytes delivered, the extension the object's key ends with, and the
/// media type the stored object carries.
struct ExportFile {
    /// The encoded bytes.
    body: Vec<u8>,
    /// The extension of the file name, naming the encoding the bytes are in.
    extension: &'static str,
    /// The media type of those bytes.
    content_type: &'static str,
}

/// The export file holding `columns`, encoded as `output` states.
///
/// The pairs served are exactly the ones this service publishes as the encodings it writes
/// ([`super::SUPPORTED_OUTPUT_ENCODINGS`]), which is what both operations that accept a
/// definition refuse a destination outside of — so a pair reaching here that is not one of
/// them is a definition no delivery can encode, reported as the delivery that did not
/// happen rather than as a file in an encoding nobody asked for.
///
/// `columns` is already held inside [`super::MAX_DELIVERED_COLUMNS`] by the read that built
/// it, which is what bounds the bytes of every body below and the schema leaves of the
/// Parquet one; what is checked here is only that there is a column at all.
///
/// `Err` carries what the encoder refused.
fn export_file(
    name: &str,
    columns: &[String],
    output: &S3OutputConfigurations,
) -> Result<ExportFile, String> {
    if columns.is_empty() {
        return Err("the data query projects no column".to_string());
    }
    if columns.len() > super::MAX_DELIVERED_COLUMNS {
        // Unreachable through `delivered_columns`, which refuses such a projection before it
        // builds the names; stated here so the encoders below cannot be reached with an
        // unbounded column list by some later caller.
        return Err(format!(
            "the data query projects {} columns, more than the {} an export file carries",
            columns.len(),
            super::MAX_DELIVERED_COLUMNS
        ));
    }
    match (output.format.as_str(), output.compression.as_str()) {
        ("TEXT_OR_CSV", "GZIP") => Ok(ExportFile {
            body: gzip(&delimited_text(columns)?)?,
            extension: "csv.gz",
            content_type: "application/gzip",
        }),
        ("TEXT_OR_CSV", "ZIP") => Ok(ExportFile {
            body: zip(
                &format!("{name}-{REPORT_PART}.csv"),
                &delimited_text(columns)?,
            )?,
            extension: "csv.zip",
            content_type: "application/zip",
        }),
        ("PARQUET", "PARQUET") => Ok(ExportFile {
            body: parquet(name, columns)?,
            extension: "snappy.parquet",
            content_type: "application/vnd.apache.parquet",
        }),
        (format, compression) => Err(format!(
            "this service writes no {format} file compressed as {compression}"
        )),
    }
}

/// The delimited text of an export file: its header row, naming the columns the query
/// projects, and one row per line the query selects — of which this emulator's cost and
/// usage data holds none.
///
/// Written through the `csv` crate, so the quoting and the line endings are the maintained
/// implementation's rather than a second rendering of the format kept here.
fn delimited_text(columns: &[String]) -> Result<Vec<u8>, String> {
    let mut writer = ::csv::Writer::from_writer(Vec::new());
    writer
        .write_record(columns)
        .map_err(|error| error.to_string())?;
    writer
        .into_inner()
        .map_err(|error| error.to_string())
        .map(|written| written.to_vec())
}

/// `body` as a GZIP stream.
fn gzip(body: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
    encoder
        .write_all(body)
        .map_err(|error| error.to_string())?;
    encoder.finish().map_err(|error| error.to_string())
}

/// `body` as a ZIP archive holding it as its single entry, under the name the uncompressed
/// file carries.
fn zip(entry: &str, body: &[u8]) -> Result<Vec<u8>, String> {
    let mut archive = ::zip::ZipWriter::new(::std::io::Cursor::new(Vec::new()));
    archive
        .start_file(entry, ::zip::write::SimpleFileOptions::default())
        .map_err(|error| error.to_string())?;
    archive
        .write_all(body)
        .map_err(|error| error.to_string())?;
    archive
        .finish()
        .map_err(|error| error.to_string())
        .map(::std::io::Cursor::into_inner)
}

/// The bytes of the Apache Parquet file of an export whose columns are `columns`: one
/// OPTIONAL UTF-8 leaf per projected column, Snappy-compressed, and the rows the query
/// selects — none, so the file carries its schema and no row group.
///
/// Written by the Apache implementation of the format through its file writer, so what lands
/// in the bucket is a Parquet file any Parquet reader reads.
fn parquet(name: &str, columns: &[String]) -> Result<Vec<u8>, String> {
    use ::parquet::basic::{Compression, ConvertedType, Repetition, Type as PhysicalType};
    use ::parquet::file::properties::WriterProperties;
    use ::parquet::file::writer::SerializedFileWriter;
    use ::parquet::schema::types::Type as SchemaType;

    let mut fields = Vec::with_capacity(columns.len());
    for column in columns {
        let field = SchemaType::primitive_type_builder(column, PhysicalType::BYTE_ARRAY)
            // Every column is optional: a cost and usage column may hold no value for a
            // line item.
            .with_repetition(Repetition::OPTIONAL)
            .with_converted_type(ConvertedType::UTF8)
            .build()
            .map_err(|error| error.to_string())?;
        fields.push(::std::sync::Arc::new(field));
    }
    let schema = SchemaType::group_type_builder(name)
        .with_fields(fields)
        .build()
        .map_err(|error| error.to_string())?;
    let properties = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let writer = SerializedFileWriter::new(
        Vec::new(),
        ::std::sync::Arc::new(schema),
        ::std::sync::Arc::new(properties),
    )
    .map_err(|error| error.to_string())?;
    // `into_inner` finalises the file — it writes the footer the schema is read back from —
    // so a file with no row group is a complete Parquet file holding no row.
    writer
        .into_inner()
        .map_err(|error| error.to_string())
        .map(|buffer| buffer.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::EXECUTION_STATUS_CODE_VALUES;
    use ::std::time::Duration;

    /// Every status the pipeline moves a delivery through is a value the MODEL publishes, so
    /// a renamed enum value cannot leave this service reporting a status no client knows.
    #[test]
    fn the_pipeline_names_only_modelled_statuses() {
        for code in DELIVERY_PIPELINE {
            assert!(
                EXECUTION_STATUS_CODE_VALUES.contains(code),
                "case {code:?}: expected a modelled ExecutionStatusCode, got one outside \
                 {EXECUTION_STATUS_CODE_VALUES:?}"
            );
        }
        for code in [
            EXECUTION_STATUS_DELIVERY_SUCCESS,
            EXECUTION_STATUS_DELIVERY_FAILURE,
        ] {
            assert!(
                EXECUTION_STATUS_CODE_VALUES.contains(&code),
                "case {code:?}: expected a modelled ExecutionStatusCode, got one outside \
                 {EXECUTION_STATUS_CODE_VALUES:?}"
            );
        }
    }

    /// The status in force walks the pipeline in order as the span elapses, and is a
    /// function of the stored instants alone: the same `now` always answers the same status,
    /// dated at the instant that status fell due.
    #[test]
    fn the_pipeline_is_walked_in_order_as_the_span_elapses() {
        let initiated_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let span = Duration::from_millis(800);
        let complete_at = initiated_at + span;
        let step = span / DELIVERY_PIPELINE.len() as u32;
        for (index, expected) in DELIVERY_PIPELINE.iter().enumerate() {
            let due_at = initiated_at + step * index as u32;
            for sampled in [due_at, due_at + step / 2] {
                let in_force = pipeline_status(initiated_at, complete_at, sampled);
                assert_eq!(
                    in_force,
                    Some((*expected, due_at)),
                    "case {index:?} at {sampled:?}: expected {expected:?} due at {due_at:?}, \
                     got {in_force:?}"
                );
            }
        }
    }

    /// A span too short to divide leaves a delivery in the status it was initiated in, and a
    /// delivery that would finish before it starts has no pipeline at all.
    #[test]
    fn a_span_with_nothing_to_divide_has_no_pipeline() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        for (case, complete_at, expected) in [
            (
                "a delivery completing when it starts",
                at,
                Some((EXECUTION_STATUS_INITIATION_IN_PROCESS, at)),
            ),
            (
                "a span shorter than one step",
                at + Duration::from_nanos(DELIVERY_PIPELINE.len() as u64 - 1),
                Some((EXECUTION_STATUS_INITIATION_IN_PROCESS, at)),
            ),
            (
                "a delivery completing before it starts",
                at - Duration::from_secs(1),
                None,
            ),
        ] {
            let in_force = pipeline_status(at, complete_at, at + Duration::from_secs(10));
            assert_eq!(
                in_force, expected,
                "case {case:?}: expected {expected:?}, got {in_force:?}"
            );
        }
    }
}
