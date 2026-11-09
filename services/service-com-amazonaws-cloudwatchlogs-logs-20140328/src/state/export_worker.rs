// SPDX-License-Identifier: CC0-1.0

//! The work an export task does: writing the log events it selected to the destination
//! bucket as Amazon S3 objects.
//!
//! `CreateExportTask` "creates an export task so that you can efficiently export data from a
//! log group to an Amazon S3 bucket", and "this is an asynchronous call" — so the call that
//! starts an export ANSWERS while the export is still owed, and the objects are what the
//! export produces. They are therefore written here, off the response path, and the task
//! RECORDS that it wrote them ([`super::ExportTaskRecord::record_export_written`]), which is
//! what lets the clock complete it: a caller that waits for `COMPLETED` and then reads the
//! destination prefix finds the exported data there, because the status says the data was
//! delivered and nothing but the delivery can produce it.
//!
//! What is written follows the task:
//!
//! * which events — the named log group's events whose timestamp falls in the task's window
//!   ("events with a timestamp before this time are not exported" / "events with a timestamp
//!   later than this time are not exported"), taken from the streams the task's
//!   `logStreamNamePrefix` selects ("export only log streams that match the provided
//!   prefix");
//! * where — under the task's `destinationPrefix`, "the prefix that was used as the start of
//!   Amazon S3 key for every object exported", one object per exported log stream, keyed by
//!   the task's own id so two exports into one prefix do not overwrite each other;
//! * how — gzip-framed text, one line per event carrying the event's own timestamp and
//!   message, which is the form CloudWatch Logs lands exported log data in.
//!
//! Amazon S3 is reached the way the framework documents a handler reaching another emulated
//! service ([`smithy_server_runtime::OperationContext::callback`]): an AWS SDK client over
//! the in-process callback's HTTP client and credentials, so nothing here opens a socket or
//! leaves the host. The one caller-supplied value that reaches the client's authority is the
//! signing Region, narrowed before it gets there ([`service_common::arn::region_label`]).
//!
//! The store's guard is never held across a write: the events are read under it, the objects
//! are written with it released, and it is taken again only to record what the export did.
//! An export whose objects Amazon S3 refused is `FAILED` — the status CloudWatch Logs
//! reports for an export that stopped on an error — rather than completed over nothing.
//!
//! The work one export does is BOUNDED by what the store can hold: at most one object per log
//! stream of the group ([`super::MAX_STREAMS_PER_GROUP`]), each built from that stream's
//! events, which the store itself caps ([`super::MAX_STREAM_EVENTS`],
//! [`super::MAX_STREAM_BYTES`]). There is no loop over the clock and no retry: the worker
//! waits once, writes once per stream, records the outcome and ends.
//!
//! Because the task's own completion waits on that outcome, every path out of the worker
//! either records one or leaves no task behind to wait for it: the record lives in the very
//! store the worker reads, so a store it cannot reach holds no task either, and a task the
//! store no longer holds has been dropped. The one task the worker deliberately leaves alone
//! is a cancelled one, which is already terminal and owes no further transition.

use std::io::Write;
use std::sync::Arc;

use ::smithy_server_runtime::EmbeddedEmulator;

use super::callback_client::callback_client;

/// How long after the task is created its objects are written: the span the task is
/// `PENDING` for, so the write happens while it is `RUNNING` — after the export has started
/// moving data, and within the further span the clock takes to complete it
/// ([`super::EXPORT_TASK_RUNNING_DELAY_MS`], [`super::EXPORT_TASK_COMPLETION_DELAY_MS`]), which
/// a write that takes longer than that pushes out rather than overrunning.
const WRITE_DELAY_MS: i64 = super::EXPORT_TASK_RUNNING_DELAY_MS;

/// The status message an export task that could not write its objects reports beside
/// `FAILED`: `ExportTaskStatus$message` is "the status message related to the status code",
/// and what there is to say about this one is that the destination did not take the data.
const WRITE_FAILED_STATUS_MESSAGE: &str = "Unable to write the exported log data to the \
                                           destination bucket";

/// The name every object of one exported log stream is numbered with. CloudWatch Logs
/// numbers the objects of a stream from zero and gzip-frames each; one export of a stream's
/// selected events is one object here, so it is the first of that numbering.
const FIRST_OBJECT_NAME: &str = "000000.gz";

/// Run the export the call that returns this accepted.
///
/// Returns at once: the export runs in a task of its own, because `CreateExportTask` answers
/// with the task's id while the export is still owed. The caller hands over its own clone of
/// the in-process callback, so the objects are written exactly where the starting call was
/// made.
pub fn start<Eng: Send + Sync + 'static>(
    engine: Arc<Eng>,
    callback: EmbeddedEmulator,
    region: String,
    task_id: String,
) {
    ::tokio::spawn(async move {
        run(&engine, &callback, &region, &task_id).await;
    });
}

/// Select the task's events, write them, and record what the write did.
async fn run<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    callback: &EmbeddedEmulator,
    region: &str,
    task_id: &str,
) {
    ::tokio::time::sleep(::std::time::Duration::from_millis(
        u64::try_from(WRITE_DELAY_MS).unwrap_or(0),
    ))
    .await;

    // The task's record lives in this very store, so a store that is not there holds no task
    // whose export is owed.
    let Some(state) = super::service_state(&**engine) else {
        return;
    };

    // Everything the write needs is read under the guard, which is then released: the
    // objects below are written with no lock held.
    let plan = {
        let mut store = state.lock();
        // The statuses the clock already owes are materialised first, so the status read
        // here is the current one.
        store.materialize_due_export_transitions(super::now_ms());
        // A task the store no longer holds was dropped from it, so there is no export left to
        // do and nothing left to record it on.
        let Some(task) = store.export_tasks.get(task_id) else {
            return;
        };
        // Only a CANCELLED task exports nothing: a caller stopped it, so there is no export
        // left to do. A task the clock has already carried past `RUNNING` is NOT one of
        // those — it is this write running late — and skipping it is what would leave a
        // terminal status standing over an unwritten export.
        if is_cancelled(&task.status_code) {
            return;
        }
        // A group that is no longer there exports nothing — there are no events left to
        // select — which is an export of an empty selection and not an export left undone.
        let objects: Vec<(String, Vec<u8>)> = store
            .log_groups
            .get(&task.log_group_name)
            .into_iter()
            .flat_map(|group| group.streams.values())
            .filter(|stream| {
                task.log_stream_name_prefix
                    .as_deref()
                    .is_none_or(|prefix| stream.name.starts_with(prefix))
            })
            .filter_map(|stream| {
                let body = exported_text(&stream.events, task.from_ms, task.to_ms);
                (!body.is_empty()).then(|| {
                    (
                        object_key(&task.destination_prefix, task_id, &stream.name),
                        gzip(&body),
                    )
                })
            })
            .collect();
        WritePlan {
            bucket: task.destination.clone(),
            objects,
        }
    };

    let client = s3_client(callback, region);
    let mut written = true;
    for (key, body) in &plan.objects {
        let answer = client
            .put_object()
            .bucket(&plan.bucket)
            .key(key)
            .body(::aws_sdk_s3::primitives::ByteStream::from(body.clone()))
            .send()
            .await;
        if let Err(error) = answer {
            // The caller's own values are rendered in their escaped `Debug` form, so a
            // bucket name or prefix carrying a newline cannot forge a log line.
            ::tracing::warn!(
                bucket = ?plan.bucket,
                key = ?key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an export task's object could not be written to Amazon S3"
            );
            written = false;
            break;
        }
    }

    // What the write did is recorded on the task, which is what lets it settle: a delivered
    // export may now be completed by the clock, and one the destination refused ends FAILED —
    // the status CloudWatch Logs reports for an export that stopped on an error — rather than
    // being completed over nothing.
    let mut store = state.lock();
    let Some(task) = store.export_tasks.get_mut(task_id) else {
        return;
    };
    if is_cancelled(&task.status_code) {
        return;
    }
    if written {
        task.record_export_written(super::now_ms());
        return;
    }
    task.status_code = super::EXPORT_TASK_STATUS_FAILED.to_string();
    task.status_message = Some(WRITE_FAILED_STATUS_MESSAGE.to_string());
    task.completion_time_ms = Some(super::now_ms());
    task.running_due_ms = None;
    task.completion_due_ms = None;
}

/// Whether `status_code` is the status of an export a caller STOPPED — the one status that
/// leaves no export to do. `CancelExportTask` moves a task straight to
/// [`super::EXPORT_TASK_STATUS_CANCELLED`], which is therefore the only stopped status a task
/// of this store ever carries.
fn is_cancelled(status_code: &str) -> bool {
    status_code == super::EXPORT_TASK_STATUS_CANCELLED
}

/// One export one worker writes: which bucket, and the objects that go to it.
#[derive(Debug)]
struct WritePlan {
    /// `ExportTask$destination` — "the name of the S3 bucket to which the log data was
    /// exported".
    bucket: String,
    /// Each object as `(key, gzip-framed body)`.
    objects: Vec<(String, Vec<u8>)>,
}

/// The S3 key of the object holding `stream_name`'s exported events.
///
/// `destination_prefix` is "the prefix that was used as the start of Amazon S3 key for every
/// object exported"; the task's own id and the stream's name follow it, so one prefix can
/// hold the output of several exports and of several streams without collision.
fn object_key(destination_prefix: &str, task_id: &str, stream_name: &str) -> String {
    let prefix = destination_prefix.trim_end_matches('/');
    if prefix.is_empty() {
        format!("{task_id}/{stream_name}/{FIRST_OBJECT_NAME}")
    } else {
        format!("{prefix}/{task_id}/{stream_name}/{FIRST_OBJECT_NAME}")
    }
}

/// The exported text of the events of one stream whose timestamp falls in `[from_ms, to_ms)`,
/// oldest first: one line per event, carrying the event's own timestamp and its message.
///
/// The window's end is EXCLUSIVE and its start inclusive, which is the boundary the recorded
/// exports show: an event stamped exactly at `to_ms` belongs to the next window, so two
/// adjacent exports neither duplicate it nor drop it.
fn exported_text(events: &[super::StoredEvent], from_ms: i64, to_ms: i64) -> String {
    let mut text = String::new();
    for event in events
        .iter()
        .filter(|event| event.timestamp >= from_ms && event.timestamp < to_ms)
    {
        text.push_str(&format_timestamp(event.timestamp));
        text.push(' ');
        text.push_str(&event.message);
        text.push('\n');
    }
    text
}

/// One exported event's timestamp, as the exported line carries it: the instant it names, in
/// UTC.
fn format_timestamp(timestamp_ms: i64) -> String {
    ::aws_smithy_types::DateTime::from_millis(timestamp_ms)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        // Every instant this type can hold renders; a timestamp that does not is written as
        // the epoch milliseconds it was ingested with rather than dropping the event.
        .unwrap_or_else(|_| timestamp_ms.to_string())
}

/// `body` in a gzip frame, the framing exported log data is written in.
fn gzip(body: &str) -> Vec<u8> {
    let mut encoder =
        ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
    // Writing to a `Vec` cannot fail, and neither can finishing the frame over one.
    let _ = encoder.write_all(body.as_bytes());
    encoder.finish().unwrap_or_default()
}

/// An Amazon S3 client over the in-process callback into this same emulator.
fn s3_client(callback: &EmbeddedEmulator, region: &str) -> ::aws_sdk_s3::Client {
    let region = ::service_common::arn::region_label(region).unwrap_or(super::DEFAULT_REGION);
    // The emulator serves one endpoint for every service, so the bucket travels in the path
    // rather than in a per-bucket host the router could not resolve.
    callback_client!(aws_sdk_s3, callback, "s3", region, force_path_style(true))
}
