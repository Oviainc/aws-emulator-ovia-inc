// SPDX-License-Identifier: CC0-1.0

//! EXPERIMENT LOGGING: the records an experiment's run DELIVERS to the destination its
//! template configured.
//!
//! A template may carry a `logConfiguration` naming a CloudWatch Logs log group
//! (`cloudWatchLogsConfiguration.logGroupArn`), an Amazon S3 destination
//! (`s3Configuration.bucketName` / `prefix`), or both, beside the `logSchemaVersion` the
//! records are written under. `StartExperiment` copies that configuration onto the
//! experiment it mints, which makes the destination "part of the experiment the service
//! runs": FIS "sends detailed experiment logs to the configured destination" while the
//! experiment runs, so a client that starts an experiment and then reads the log group
//! (`logs:FilterLogEvents`) or lists the bucket (`s3:ListObjectsV2`) finds the run's
//! records there. Storing the configuration and echoing it back is NOT the behaviour —
//! the destination has to receive something.
//!
//! # What is delivered, and what is not
//!
//! A record is produced for each event of the experiment's own LIFECYCLE, which is what
//! this emulator models (see `crate::state`'s module header):
//!
//! * [`LOG_TYPE_EXPERIMENT_START`] — the run was accepted and is starting up;
//! * [`LOG_TYPE_TARGET_RESOLUTION`] — one record per target the start resolved, carrying
//!   what it resolved to (`crate::state::resolve_targets`);
//! * [`LOG_TYPE_EXPERIMENT_STATE_CHANGE`] — every move the clock made the experiment's
//!   status take (`initiating` -> `running`, `running` -> `stopping`), stamped at the
//!   instant that move fell due rather than at whoever observed it;
//! * [`LOG_TYPE_EXPERIMENT_END`] — the move into a terminal status, carrying the reason
//!   and, for a failed run, the `ExperimentError` the service reports for it.
//!
//! No per-action record is produced, for exactly the reason the experiment's own
//! `actions[].state` / `startTime` / `endTime` are left unset (`crate::state`'s
//! `experiment_action`): this emulator models the experiment's lifecycle, not the effect
//! an action has on another service, so nothing is logged as having been carried out that
//! nothing carried out.
//!
//! # The record's form
//!
//! Each record is one JSON object: the experiment's `id`, the `log_type` above, the
//! `event_timestamp` the event fell due at (RFC 3339, UTC), the `log_schema_version` the
//! template configured — echoed, never invented — and a `details` object built from the
//! experiment's own persisted state. AWS publishes the member set of its experiment log
//! schema in the FIS User Guide rather than in this service's Smithy model, and no
//! recording here pins it, so the member names are this emulator's rendering of the
//! record; what the rule requires, and what is asserted, is that a record per lifecycle
//! event reaches the configured destination carrying the experiment it belongs to, the
//! event, its instant and the configured schema version.
//!
//! # When it is delivered
//!
//! A record is PRODUCED by the same state transition that makes it true — the start that
//! mints the experiment, or [`crate::state::ExperimentRecord::settle`] materialising a
//! move the clock already owed — and parked in an outbox on the experiment record
//! ([`crate::state::ExperimentRecord::pending_log_records`], bounded by
//! [`MAX_PENDING_LOG_RECORDS`]). An operation that has just settled the store then drains
//! the outbox ([`crate::state::ComAmazonawsFisFaultinjectionsimulatorState::take_pending_log_deliveries`])
//! and writes what it drained, with the state guard already released. Nothing sleeps for a
//! delivery and nothing is delivered before the event it reports fell due.
//!
//! Delivery is BEST EFFORT, as it is for the real service: a destination that does not
//! resolve (no such log group, no such bucket) or refuses the write loses the records and
//! is reported in this service's own tracing, exactly as a destination FIS cannot write to
//! loses them — it never fails the FIS call that happened to drain the outbox, which
//! asked for none of this.
//!
//! The writes go through the emulator's OWN CloudWatch Logs and Amazon S3 services over
//! the in-process callback, which is the framework's seam for calling another emulated
//! service: the records land in the very stores `logs:GetLogEvents` and
//! `s3:ListObjectsV2` read, rather than in a second copy of them. The S3 client is the
//! crate's one builder ([`crate::state`]'s `s3_client`), shared with the experiment report.
//! No network client is configured and no outbound connection is made.

use std::time::SystemTime;

use crate::ops::types::ResolvedTarget;

/// The `log_type` of the record a started experiment produces.
pub const LOG_TYPE_EXPERIMENT_START: &str = "experiment-start";

/// The `log_type` of the record produced for each target the start resolved.
pub const LOG_TYPE_TARGET_RESOLUTION: &str = "target-resolution";

/// The `log_type` of the record produced for a status change that is not the run's end.
pub const LOG_TYPE_EXPERIMENT_STATE_CHANGE: &str = "experiment-state-change";

/// The `log_type` of the record produced when the experiment reaches a terminal status.
pub const LOG_TYPE_EXPERIMENT_END: &str = "experiment-end";

/// The media type an experiment log record is written to Amazon S3 with: each object holds
/// one JSON record.
pub const LOG_RECORD_CONTENT_TYPE: &str = "application/json";

/// How many undelivered records ONE experiment's outbox holds.
///
/// A whole run's records are a start record, ONE PER RESOLVED TARGET — bounded by
/// [`crate::state::MAX_RESOLVED_TARGETS`], the ceiling the resolution itself is computed
/// to — and at most one per status change, of which a run has at most three. The bound is
/// therefore that resolution ceiling plus [`LIFECYCLE_LOG_RECORDS`], so a run NEVER loses
/// a record of a target it resolved: a bound below the resolution's own would silently
/// drop the tail of a large template's resolution.
///
/// What it still bounds is an experiment nobody reads: the records of one run and no more
/// are held, whatever the run is, so the memory is a constant times the resolution the
/// store already holds rather than something a caller can grow without limit. Past it a
/// record is dropped rather than queued, and the drop is traced
/// ([`crate::state::ExperimentRecord`]'s `log_event`).
pub const MAX_PENDING_LOG_RECORDS: usize =
    crate::state::MAX_RESOLVED_TARGETS + LIFECYCLE_LOG_RECORDS;

/// How many records one run's own lifecycle produces beside its target resolution: the
/// start, and one per status change the clock can make (`initiating` -> `running`,
/// `running` -> terminal, and the `stopping` a `StopExperiment` inserts between them).
pub const LIFECYCLE_LOG_RECORDS: usize = 4;

/// The ARN segment that introduces a log group's name
/// (`arn:<partition>:logs:<region>:<account>:log-group:<name>`).
const LOG_GROUP_ARN_SEGMENT: &str = ":log-group:";

/// The most bytes ONE `PutLogEvents` batch may carry, from that operation's documented
/// quota: "The maximum batch size is 1,048,576 bytes."
const MAX_LOG_EVENT_BATCH_BYTES: usize = 1_048_576;

/// What that quota charges per event beside its message: the batch size "is calculated as
/// the sum of all event messages in UTF-8, plus 26 bytes for each log event".
const LOG_EVENT_OVERHEAD_BYTES: usize = 26;

/// The most events ONE `PutLogEvents` batch may carry, from the same quota: "The maximum
/// number of log events in a batch is 10,000."
const MAX_LOG_EVENTS_PER_BATCH: usize = 10_000;

/// The most bytes ONE log event's message may carry, from the same quota: "The maximum
/// size of a log event is 256 KB." A record past it is refused by CloudWatch Logs, so it
/// is left out of the batch rather than taking the batch down with it.
const MAX_LOG_EVENT_BYTES: usize = 256 * 1024;

/// One experiment log record the run has produced and not yet delivered.
///
/// It carries the event and the instant the event fell due; the experiment it belongs to
/// and the schema version it is written under come from the record it is parked on, so a
/// record cannot disagree with the experiment's own configuration.
#[derive(Debug, Clone)]
pub struct LogRecord {
    /// One of the `LOG_TYPE_*` constants above.
    pub log_type: &'static str,
    /// When the event this record reports fell due — NOT when it was delivered.
    pub at: SystemTime,
    /// What the event says, built from the experiment's persisted state.
    pub details: ::serde_json::Value,
}

/// One experiment's drained outbox, with everything delivering it needs.
///
/// Taken out of the store as an owned value so the state guard is released before any
/// delivery is attempted: nothing in this module holds a lock across an `await`.
#[derive(Debug, Clone)]
pub struct PendingDelivery {
    /// The experiment these records belong to — the `id` each record carries, and the name
    /// of the CloudWatch Logs stream they are written to.
    pub experiment_id: String,
    /// `logConfiguration.logSchemaVersion`, echoed onto every record.
    pub log_schema_version: Option<i32>,
    /// `logConfiguration.cloudWatchLogsConfiguration.logGroupArn`, when the template
    /// configured one.
    pub log_group_arn: Option<String>,
    /// `logConfiguration.s3Configuration.bucketName`, when the template configured one.
    pub bucket_name: Option<String>,
    /// `logConfiguration.s3Configuration.prefix`.
    pub prefix: Option<String>,
    /// How many records this experiment had already produced before these: it is what
    /// numbers the S3 object keys, so the keys of one experiment's records sort in the
    /// order the run produced them however many drains it took.
    pub first_sequence: usize,
    /// The records, in the order the run produced them.
    pub records: Vec<LogRecord>,
}

/// The `details` of a [`LOG_TYPE_TARGET_RESOLUTION`] record: what one target resolved to.
pub fn target_resolution_details(target: &ResolvedTarget) -> ::serde_json::Value {
    let mut details = ::serde_json::Map::new();
    if let Some(name) = &target.target_name {
        details.insert("target_name".to_string(), ::serde_json::json!(name));
    }
    if let Some(resource_type) = &target.resource_type {
        details.insert("resource_type".to_string(), ::serde_json::json!(resource_type));
    }
    if let Some(information) = &target.target_information {
        // A `BTreeMap` rendering, so one target's information is written in a stable
        // order rather than in a hash map's.
        let ordered: ::std::collections::BTreeMap<&String, &String> = information.iter().collect();
        details.insert(
            "target_information".to_string(),
            ::serde_json::json!(ordered),
        );
    }
    ::serde_json::Value::Object(details)
}

/// The record as it is written to a destination.
///
/// A member the experiment does not carry is LEFT OUT rather than written as `null`: the
/// `logSchemaVersion` is the template's, so a configuration that named none contributes
/// none.
pub fn rendered(
    record: &LogRecord,
    experiment_id: &str,
    log_schema_version: Option<i32>,
) -> String {
    let mut envelope = ::serde_json::Map::new();
    envelope.insert("id".to_string(), ::serde_json::json!(experiment_id));
    envelope.insert("log_type".to_string(), ::serde_json::json!(record.log_type));
    envelope.insert(
        "event_timestamp".to_string(),
        ::serde_json::json!(rfc3339(record.at)),
    );
    if let Some(version) = log_schema_version {
        envelope.insert(
            "log_schema_version".to_string(),
            ::serde_json::json!(version),
        );
    }
    envelope.insert("details".to_string(), record.details.clone());
    ::serde_json::Value::Object(envelope).to_string()
}

/// `at` as an RFC 3339 UTC timestamp, or the empty string for an instant no timestamp can
/// hold.
pub fn rfc3339(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// `at` in milliseconds since the Unix epoch, which is the stamp CloudWatch Logs takes.
///
/// An instant before the epoch, which no log event can carry, clamps to the epoch rather
/// than wrapping.
pub fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// The log group name `arn` addresses, or `None` when it addresses no log group.
///
/// `CloudWatchLogGroupArn` is only constrained by `@length(20..2048)` / `^[\S]+$`, so the
/// group is read out of the ARN's own form:
/// `arn:<partition>:logs:<region>:<account>:log-group:<name>`, optionally followed by the
/// `:*` suffix CloudWatch Logs reports for a group and the FIS documentation shows on a
/// logging destination. A log group name admits none of `:` (its own
/// `@pattern [\.\-_/#A-Za-z0-9]+`), so the name ends at the next colon.
pub fn log_group_name(arn: &str) -> Option<&str> {
    let after = arn.split_once(LOG_GROUP_ARN_SEGMENT)?.1;
    let name = after.split(':').next().unwrap_or(after);
    (!name.is_empty()).then_some(name)
}

/// The S3 key one record is written to: `<prefix>/<experiment id>/<sequence>-<log type>.json`.
///
/// The sequence is the record's index in the experiment's own record stream, zero-padded,
/// so the keys of one experiment's records sort in the order the run produced them — which
/// is what makes a prefix listing of a bucket read as a run. A configuration that named no
/// prefix, or named one that is only separators, writes directly under the experiment.
///
/// Four digits is enough for that order to hold: one run's records are bounded by
/// [`MAX_PENDING_LOG_RECORDS`] (the resolution's own ceiling plus the run's lifecycle
/// records), which is an order of magnitude below the first five-digit sequence — past
/// which the lexicographic order would stop being the production order.
pub fn object_key(
    prefix: Option<&str>,
    experiment_id: &str,
    sequence: usize,
    log_type: &str,
) -> String {
    let stem = format!("{experiment_id}/{sequence:04}-{log_type}.json");
    match prefix
        .map(|prefix| prefix.trim_matches('/'))
        .filter(|prefix| !prefix.is_empty())
    {
        Some(prefix) => format!("{prefix}/{stem}"),
        None => stem,
    }
}

/// A CloudWatch Logs client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it routes in process, so no network client is configured here and this crate names no
/// other service's crate. `region` is the request's own signing region, so the records
/// land where the experiment's ARN says the run happened.
fn logs_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_cloudwatchlogs::Client {
    let config = ::aws_sdk_cloudwatchlogs::Config::builder()
        .behavior_version(::aws_sdk_cloudwatchlogs::config::BehaviorVersion::latest())
        .region(::aws_sdk_cloudwatchlogs::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!("https://logs.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly. (The configuration CloudWatch RUM's and b2bi's own callback
        // clients use.)
        .retry_config(::aws_sdk_cloudwatchlogs::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_cloudwatchlogs::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_cloudwatchlogs::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_cloudwatchlogs::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_cloudwatchlogs::Client::from_conf(config)
}

/// Write every drained record to the destination the experiment it belongs to was
/// configured with.
///
/// Best effort, and never fatal: a destination that does not resolve or refuses the write
/// is traced and the next one is still written, so the FIS call that drained the outbox is
/// answered either way. Call it with NO state guard held — it awaits.
pub async fn deliver(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    deliveries: Vec<PendingDelivery>,
) {
    for delivery in deliveries {
        if delivery.records.is_empty() {
            continue;
        }
        if let Some(arn) = delivery.log_group_arn.clone() {
            deliver_to_log_group(ctx, region, &delivery, &arn).await;
        }
        if let Some(bucket) = delivery.bucket_name.clone() {
            deliver_to_bucket(ctx, region, &delivery, &bucket).await;
        }
    }
}

/// Write one experiment's drained records to its CloudWatch Logs log group, in a stream of
/// the experiment's own name.
///
/// The stream is created first (a stream that is already there is the one this experiment
/// has been writing to), and the batch is ordered by the instant each event fell due,
/// which is what `PutLogEvents` requires of a batch.
async fn deliver_to_log_group(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    delivery: &PendingDelivery,
    log_group_arn: &str,
) {
    let Some(log_group) = log_group_name(log_group_arn) else {
        ::tracing::warn!(
            experiment_id = %delivery.experiment_id,
            log_group_arn = %log_group_arn,
            "the configured experiment log destination names no CloudWatch Logs log group"
        );
        return;
    };
    let client = logs_client(ctx, region);
    match client
        .create_log_stream()
        .log_group_name(log_group)
        .log_stream_name(&delivery.experiment_id)
        .send()
        .await
    {
        Ok(_) => {}
        Err(error)
            if error
                .as_service_error()
                .is_some_and(|service| service.is_resource_already_exists_exception()) => {}
        Err(error) => {
            ::tracing::warn!(
                experiment_id = %delivery.experiment_id,
                log_group = %log_group,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the experiment's log stream could not be created, so its records are not delivered"
            );
            return;
        }
    }

    let mut ordered: Vec<&LogRecord> = delivery.records.iter().collect();
    ordered.sort_by_key(|record| record.at);
    let events: Vec<::aws_sdk_cloudwatchlogs::types::InputLogEvent> = ordered
        .into_iter()
        .filter_map(|record| {
            let message = rendered(
                record,
                &delivery.experiment_id,
                delivery.log_schema_version,
            );
            // "Each log event can be no larger than 256 KB" — a record past the
            // per-event ceiling is one CloudWatch Logs refuses, so it is left out
            // rather than taking the whole batch down with it.
            if message.len() > MAX_LOG_EVENT_BYTES {
                ::tracing::warn!(
                    experiment_id = %delivery.experiment_id,
                    log_type = %record.log_type,
                    bytes = message.len(),
                    limit = MAX_LOG_EVENT_BYTES,
                    "an experiment log record is larger than one log event may be, so it \
                     is not delivered to CloudWatch Logs"
                );
                return None;
            }
            ::aws_sdk_cloudwatchlogs::types::InputLogEvent::builder()
                .timestamp(epoch_millis(record.at))
                .message(message)
                .build()
                .ok()
        })
        .collect();
    // The records go out in BATCHES within the documented limits — "The maximum batch
    // size is 1,048,576 bytes[, which] is calculated as the sum of all event messages in
    // UTF-8, plus 26 bytes for each log event" and at most 10,000 events — so a run whose
    // records exceed one batch delivers all of them instead of having the one oversized
    // batch refused. Each batch stays in the order the run produced them, which is the
    // chronological order `PutLogEvents` requires.
    let mut batch: Vec<::aws_sdk_cloudwatchlogs::types::InputLogEvent> = Vec::new();
    let mut batch_bytes = 0usize;
    for event in events {
        let cost = event.message().len() + LOG_EVENT_OVERHEAD_BYTES;
        if !batch.is_empty()
            && (batch.len() >= MAX_LOG_EVENTS_PER_BATCH
                || batch_bytes + cost > MAX_LOG_EVENT_BATCH_BYTES)
        {
            put_log_events(&client, log_group, &delivery.experiment_id, batch).await;
            batch = Vec::new();
            batch_bytes = 0;
        }
        batch_bytes += cost;
        batch.push(event);
    }
    if !batch.is_empty() {
        put_log_events(&client, log_group, &delivery.experiment_id, batch).await;
    }
}

/// Send ONE batch of events to `log_group`/`log_stream`, tracing a refusal rather than
/// failing the call that drained them. See [`deliver_to_log_group`] for the batching.
async fn put_log_events(
    client: &::aws_sdk_cloudwatchlogs::Client,
    log_group: &str,
    log_stream: &str,
    events: Vec<::aws_sdk_cloudwatchlogs::types::InputLogEvent>,
) {
    let count = events.len();
    if let Err(error) = client
        .put_log_events()
        .log_group_name(log_group)
        .log_stream_name(log_stream)
        .set_log_events(Some(events))
        .send()
        .await
    {
        ::tracing::warn!(
            log_stream = %log_stream,
            log_group = %log_group,
            events = count,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the experiment's log records could not be delivered to CloudWatch Logs"
        );
    }
}

/// Write one experiment's drained records to its Amazon S3 destination, one object per
/// record under the configured prefix ([`object_key`]).
///
/// The client is the crate's ONE Amazon S3 client over the in-process callback
/// ([`crate::state`]'s `s3_client`), the same one an experiment's report is written
/// through: one client builder, however many things this service writes to object storage.
async fn deliver_to_bucket(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    delivery: &PendingDelivery,
    bucket: &str,
) {
    let client = super::s3_client(ctx, region);
    for (offset, record) in delivery.records.iter().enumerate() {
        let key = object_key(
            delivery.prefix.as_deref(),
            &delivery.experiment_id,
            delivery.first_sequence + offset,
            record.log_type,
        );
        let body = rendered(
            record,
            &delivery.experiment_id,
            delivery.log_schema_version,
        );
        if let Err(error) = client
            .put_object()
            .bucket(bucket)
            .key(&key)
            .content_type(LOG_RECORD_CONTENT_TYPE)
            .body(::aws_sdk_s3::primitives::ByteStream::from(
                body.into_bytes(),
            ))
            .send()
            .await
        {
            ::tracing::warn!(
                experiment_id = %delivery.experiment_id,
                bucket = %bucket,
                key = %key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an experiment log record could not be delivered to Amazon S3"
            );
        }
    }
}
