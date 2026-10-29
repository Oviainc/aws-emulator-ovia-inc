// SPDX-License-Identifier: CC0-1.0

//! The POLLER behind an event source mapping whose event source is an Amazon SQS queue.
//!
//! `CreateEventSourceMapping` "creates a mapping between an event source and an Lambda
//! Event Function. Lambda reads items from the event source and invokes the function", and
//! `Enabled` is "when true, the event source mapping is active. When false, Lambda pauses
//! polling and invocation" — so a mapping that reports `Enabled` is one that READS from its
//! source. ONE pass of this poller is the whole of what such a mapping does with no client
//! action at all:
//!
//! * POLL. A batch of at most `BatchSize` messages is taken out of the queue the mapping
//!   names, through the QUEUE's own delivery bookkeeping
//!   (`service_com_amazonaws_sqs_amazonsqs::state::Queue::hand_over`), the one path every
//!   consumer of a queue goes through, so a delivery the poller makes counts exactly as a
//!   client's `ReceiveMessage` does: the message's `ApproximateReceiveCount` rises, it is
//!   hidden for the queue's `VisibilityTimeout` and it becomes visible again when that
//!   lapses. A poll also applies what the queue's own `RedrivePolicy` owes first, so a
//!   message that has had every delivery that policy allows is moved to the dead-letter
//!   queue the policy names instead of being handed over again.
//! * FILTER. `FilterCriteria` is "an object that defines the filter criteria that determine
//!   whether Lambda should process an event", so a polled message that matches none of the
//!   mapping's patterns is not handed to the function. On a QUEUE source such a message is
//!   also removed from the queue, because nothing else would: leaving it would hand it back
//!   to the same filter at every poll until the queue gave up on it.
//! * INVOKE. The messages the filter admits are handed to the mapping's function as ONE
//!   event, over the in-process callback into the `Invoke` surface of this same emulator
//!   ([`smithy_server_runtime::EmbeddedEmulator`]), so nothing is delivered that this
//!   emulator does not serve itself.
//! * DELETE. A message leaves the queue only once the invocation it was handed to has
//!   succeeded. A batch whose invocation did not succeed is left exactly where it was: its
//!   messages come back when their visibility timeout lapses, which is precisely the case a
//!   queue's `RedrivePolicy` exists for, and the mapping reports what the attempt came to as
//!   its `LastProcessingResult`.
//! * METER. While the mapping's `MetricsConfig` selects `EventCount`, what the pass did with
//!   its events — how many it polled, how many the filter rejected, how many an invocation
//!   accepted and how many it deleted — is published for that mapping to CloudWatch (see
//!   [`super::metrics`]), including a 0-valued poll count for a minute in which the mapping
//!   polled nothing.
//!
//! What this emulator cannot do is RUN a function's deployment package: there is no
//! in-process runtime for one, which is what `Invoke` answers 501 for. So an invocation of
//! such a function is refused, the batch is not deleted, and the counts the pass publishes
//! say exactly that — the events were polled, none was invoked and none was deleted. Nothing
//! here reports a success, or counts an event as processed, that no invocation earned.
//!
//! # How a poll is driven
//!
//! Nothing sleeps inside a request. The poll loop is a background task claimed once per
//! store ([`drive_sqs_pollers`], armed by the calls that make or change a mapping) which
//! polls, hands the runtime back for [`POLL_INTERVAL`] and polls again, and stands down as
//! soon as the store holds no enabled queue mapping — so its lifetime is bounded by the
//! resource it serves rather than by a timer that would leave a mapping reporting `Enabled`
//! while nothing polls it. One wake-up's work is bounded too: at most one batch per mapping,
//! and the store holds at most [`super::MAX_EVENT_SOURCE_MAPPINGS`] mappings. With no async
//! runtime to spawn onto there is no task, and [`poll_due`] — which is the whole of one
//! wake-up's work and takes the instant it is working at — can be awaited directly instead.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::serde_json::{json, Value};
use ::service_com_amazonaws_sqs_amazonsqs as sqs;

use super::event_source_poller::Clients;
use super::metrics::{EventCounts, FunctionMetrics};
use super::{
    service_state, ComAmazonawsLambdaAwsgirapiserviceState, EVENT_SOURCE_MAPPING_STATE_ENABLED,
    EVENT_SOURCE_VENDOR_QUEUE, LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED,
    LAST_PROCESSING_RESULT_OK, STATE_ACTIVE,
};

/// How long the poll loop waits between two polls of a mapping's queue.
///
/// An EMULATOR-CHOSEN span, like the other spans this crate owns: Lambda publishes no poll
/// frequency for an event source mapping, only that the mapping polls while it is enabled.
/// It is deliberately non-zero, so the loop hands the runtime back between wake-ups instead
/// of spinning, and short enough that a caller watching its queue sees the mapping consume.
pub const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// The most messages this emulator takes out of a queue in ONE batch, whatever `BatchSize`
/// the mapping configures.
///
/// This is the EMULATOR's bound, not an AWS number (`BatchSize` reaches 10,000 for a
/// standard queue): the whole batch is held in memory and becomes one invocation payload, so
/// the count a caller chose is clamped before anything is polled, exactly as the reader of a
/// shard source clamps it. A mapping configured above it polls this many messages per batch
/// instead, which is a smaller batch and never a message that is not polled.
const MAX_MESSAGES_PER_BATCH: usize = 1_000;

/// The `eventSource` each record of the event a queue mapping hands its function carries.
const SQS_EVENT_SOURCE: &str = "aws:sqs";

/// How much work ONE pass may spend deciding which of its messages its mapping's filter
/// criteria admit, in the steps the shared matcher charges.
///
/// The pass is background work on a batch bounded by `BatchSize` against at most the
/// patterns a mapping may carry, so this is the ONE explicit cap on the product of the two.
const MAX_FILTER_STEPS_PER_BATCH: u64 = ::service_common::event_pattern::MAX_MATCH_STEPS_PER_REQUEST;

/// How often a mapping whose batch keeps failing says so: the first failure, and every this
/// many after it, so one mapping nothing can process cannot fill the log.
const FAILURE_LOG_INTERVAL: u32 = 20;

/// How long one minute is, in seconds: the window an idle mapping publishes at most one
/// 0-valued poll count in. A series published at the regular storage resolution is
/// aggregated per minute ([`super::metrics::STANDARD_STORAGE_RESOLUTION_SECONDS`]), so one
/// zero per minute is what a reader of such a series can tell apart.
const SECONDS_PER_MINUTE: f64 = 60.0;

/// One mapping's queue, as the poll loop reads it out of the store before it touches a
/// queue: the mapping to record the attempt against, the queue to poll, the function the
/// batch is handed to, how many messages one batch may hold, and what the mapping asked be
/// filtered and metered.
struct PollTarget {
    /// `UUID`, which is the key the mapping is stored under and the dimension its own
    /// series are reported under.
    uuid: String,
    /// `EventSourceArn`, the queue to take the batch out of.
    queue_arn: String,
    /// The Region that queue lives in, which is the Region this pass reaches this
    /// emulator's services in: a resource belongs to one Region's namespace.
    region: String,
    /// `FunctionArn`: "The ARN of the Lambda function" the batch is handed to.
    function_arn: String,
    /// `BatchSize`: "the maximum number of records in each batch that Lambda pulls from
    /// your stream or queue".
    batch_size: usize,
    /// The `Pattern` of each of the mapping's `FilterCriteria` filters, as stored.
    filters: Vec<String>,
    /// Whether the mapping's `MetricsConfig` selects the event-count family.
    meters_event_counts: bool,
}

/// One message this pass polled: the message itself, and the record the event handed to the
/// function carries it as.
struct PolledMessage {
    /// The id the queue holds the message under, which is what a delete names it by.
    message_id: String,
    /// The record envelope, as the function receives it.
    record: Value,
}

/// What ONE poll loop carries between its wake-ups.
///
/// Nothing in it is state a client can read: it is the loop's own bookkeeping — the clients
/// it reaches this emulator through, and per mapping how recently it published an idle poll
/// count and how many attempts in a row have failed, which is what keeps one zero per minute
/// and one log line per twenty failures rather than per wake-up.
#[derive(Default)]
pub struct PollSession {
    /// The clients this pass invokes a function through, built on first use.
    clients: Clients,
    /// For each mapping, the minute (epoch seconds divided by sixty) its last 0-valued poll
    /// count was published in.
    idle_minute: BTreeMap<String, i64>,
    /// For each mapping, how many attempts in a row have failed.
    failures: BTreeMap<String, u32>,
}

impl ::std::fmt::Debug for PollSession {
    /// The bookkeeping, without the clients: an SDK client has no rendering of its own.
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("PollSession")
            .field("idle_minute", &self.idle_minute)
            .field("failures", &self.failures)
            .finish_non_exhaustive()
    }
}

/// Start the background poll loop for this engine's mappings, if one is not already running.
///
/// Called by the operations that make or change a mapping. At most one loop runs per store
/// (the store carries the claim), it polls for as long as the store holds a mapping that
/// wants polling and stands down when it does not, and with no async runtime to spawn onto
/// there is no loop — a caller can then drive the same work through [`poll_due`].
pub fn drive_sqs_pollers<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &Arc<Eng>,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = claim_poller(&mut state.lock()) else {
        return;
    };
    let callback = ctx.callback().clone();
    // The CloudWatch store this loop's work is published to, resolved once because the
    // handle does not change.
    let metrics = FunctionMetrics::new(super::metrics::cloudwatch_state(&**engine));
    let engine = Arc::clone(engine);
    let claimed = ::engine::ServiceState::clone(&state);
    let handle = runtime.spawn(async move {
        let mut session = PollSession::default();
        // The loop ends when nothing polls any more, which it observes — and releases its
        // claim in — inside one critical section.
        while polling_wanted_or_release(&mut state.lock(), generation) {
            // Every wake-up waits, which is what hands the runtime back between polls. No
            // lock is held across it.
            ::tokio::time::sleep(POLL_INTERVAL).await;
            poll_due(
                &callback,
                &mut session,
                &*engine,
                &metrics,
                SystemTime::now(),
            )
            .await;
        }
    });
    // The handle is kept so the loop it replaces is aborted rather than merely forgotten.
    set_poller(&mut claimed.lock(), generation, handle);
}

/// One wake-up's work: poll every enabled queue mapping this engine holds, at `now`.
///
/// For each such mapping, in turn: whatever the clock already owed on the queues is applied,
/// the queue's `RedrivePolicy` is applied to the messages that have had every delivery it
/// allows, a batch of at most the mapping's `BatchSize` messages is handed over, the
/// mapping's filter criteria decide which of them the function is invoked with, and what the
/// pass did is metered. See this module's documentation for what each step is, and why a
/// batch no invocation accepted is left on the queue.
///
/// The two stores are never locked at the same time, and no lock is held across an
/// invocation: each step takes one lock, finishes with it and releases it before the next
/// step takes the other.
pub async fn poll_due<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    session: &mut PollSession,
    engine: &Eng,
    metrics: &FunctionMetrics,
    now: SystemTime,
) {
    let Some(mappings) = service_state(engine) else {
        return;
    };
    let Some(queues) = sqs::state::service_state(engine) else {
        return;
    };
    let targets = {
        let mut store = mappings.lock();
        store.materialize_all(now);
        polling_targets(&store)
    };
    let at_secs = epoch_secs(now);
    for target in targets {
        let polled = take_batch(&queues, &target, now);
        if polled.is_empty() {
            // Nothing was waiting, so no processing attempt was made and there is no result
            // of one to report. The poll itself is still a measurement: a mapping that
            // polled an empty queue says so, at most once a minute.
            if target.meters_event_counts && claim_idle_minute(session, &target.uuid, at_secs) {
                metrics.publish_idle_poll(&target.uuid, at_secs);
            }
            continue;
        }
        let (invoking, filtered_out) = partition_by_filters(&target, &polled);
        // A message the filter rejected is removed from the queue: nothing else would, and
        // the mapping would otherwise be handed it again at every poll.
        delete_messages(&queues, &target, &filtered_out);
        let mut counts = EventCounts {
            polled: count_of(polled.len()),
            filtered_out: count_of(filtered_out.len()),
            invoked: 0,
            deleted: count_of(filtered_out.len()),
        };
        if invoking.is_empty() {
            // The filter rejected every message this pass polled, so the pass handled the
            // whole batch and there is no invocation to make for it: that is a processing
            // attempt that succeeded.
            record_attempt(&mappings, &target, LAST_PROCESSING_RESULT_OK);
        } else {
            let payload = queue_event(&invoking, &polled);
            match invoke_function(callback, session, &target, &payload).await {
                Ok(()) => {
                    // The batch was processed, so its messages leave the queue — that is
                    // what a consumer of a queue does with a message it has handled.
                    counts.invoked = count_of(invoking.len());
                    delete_messages(&queues, &target, &invoking);
                    counts.deleted = counts.deleted.saturating_add(counts.invoked);
                    session.failures.remove(&target.uuid);
                    record_attempt(&mappings, &target, LAST_PROCESSING_RESULT_OK);
                }
                Err(detail) => {
                    // Not processed: nothing is deleted, so every message comes back when
                    // its visibility timeout lapses.
                    let failures = session
                        .failures
                        .entry(target.uuid.clone())
                        .and_modify(|failures| *failures = failures.saturating_add(1))
                        .or_insert(1);
                    if *failures == 1 || failures.is_multiple_of(FAILURE_LOG_INTERVAL) {
                        ::tracing::warn!(
                            mapping = %target.uuid,
                            attempt = *failures,
                            detail = %detail,
                            "an event source mapping could not hand a batch from its queue \
                             to its function; the messages stay on the queue and it tries \
                             again"
                        );
                    }
                    record_attempt(
                        &mappings,
                        &target,
                        LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED,
                    );
                }
            }
        }
        if target.meters_event_counts {
            metrics.publish_event_counts(&target.uuid, &counts, at_secs);
            // This pass polled, so the next idle minute is one to publish a zero for.
            session.idle_minute.remove(&target.uuid);
        }
    }
}

/// Take the next batch out of `target`'s queue at `now`, as the records the function would
/// be handed them as.
///
/// Empty when the mapping names a queue this engine no longer holds, or when the queue had
/// nothing it could hand over.
fn take_batch(
    queues: &::engine::ServiceState<sqs::state::ComAmazonawsSqsAmazonsqsState>,
    target: &PollTarget,
    now: SystemTime,
) -> Vec<PolledMessage> {
    let mut store = queues.lock();
    // Retention lapses, visibility timeouts and message movement tasks are owed by the
    // clock, so what the delivery below sees is what the queue may hand out.
    store.materialise_due(now);
    let Some((region, name, visibility_timeout)) =
        store.queue_by_arn(&target.queue_arn).map(|queue| {
            (
                queue.region.clone(),
                queue.name.clone(),
                queue.integer(sqs::state::VISIBILITY_TIMEOUT),
            )
        })
    else {
        return Vec::new();
    };
    // A message this queue has already handed over as many times as its `RedrivePolicy`
    // allows is not handed over again: it goes to the dead-letter queue that policy names,
    // BEFORE this poll decides what there is to deliver.
    store.dead_letter_due(&region, &name, now);
    let Some(queue) = store.queue_by_arn_mut(&target.queue_arn) else {
        return Vec::new();
    };
    // The batch is hidden for the queue's own `VisibilityTimeout`, which is the span a
    // message is handed to ONE consumer for, and which `CreateEventSourceMapping` already
    // held this pair to.
    let handed = queue.hand_over(
        now,
        target.batch_size,
        sqs::state::after(now, visibility_timeout),
    );
    handed
        .iter()
        .filter_map(|(index, delivered)| {
            let message = queue.messages.get(*index)?;
            Some(PolledMessage {
                message_id: delivered.message_id.clone(),
                record: queue_record(message, &delivered.receipt_handle, &target.queue_arn, &region),
            })
        })
        .collect()
}

/// Split `polled` into the messages `target`'s filter criteria admit and the ones they
/// reject, by message id.
///
/// A mapping carrying no criteria admits every message — there is nothing to determine
/// otherwise. A mapping whose criteria this emulator cannot read also admits every message:
/// a message rejected on a pattern nothing could evaluate would be deleted from the queue on
/// the strength of a filter that was never applied, so the reading failure withholds the
/// filter rather than the data.
fn partition_by_filters(
    target: &PollTarget,
    polled: &[PolledMessage],
) -> (Vec<String>, Vec<String>) {
    let admit_all = || {
        (
            polled
                .iter()
                .map(|message| message.message_id.clone())
                .collect(),
            Vec::new(),
        )
    };
    if target.filters.is_empty() {
        return admit_all();
    }
    let Some(patterns) = parsed_filters(&target.filters) else {
        ::tracing::warn!(
            mapping = %target.uuid,
            "an event source mapping carries filter criteria this emulator cannot read, so \
             every message it polls is handed to its function"
        );
        return admit_all();
    };
    let mut budget = ::service_common::event_pattern::MatchBudget::new(MAX_FILTER_STEPS_PER_BATCH);
    let mut admitted: Vec<String> = Vec::new();
    let mut rejected: Vec<String> = Vec::new();
    for message in polled {
        let event = filter_event(&message.record);
        // "The collection of event patterns used to filter events": a record matching ANY
        // of them is one the function is invoked with.
        let matched = patterns.iter().any(|pattern| {
            ::service_common::event_pattern::matches_within(pattern, &event, &mut budget)
        });
        if matched {
            admitted.push(message.message_id.clone());
        } else {
            rejected.push(message.message_id.clone());
        }
    }
    (admitted, rejected)
}

/// The mapping's filter patterns, parsed by the ONE reading of the event-pattern language in
/// this tree — `None` when any of them is not a pattern this emulator can match, because a
/// filter is a conjunction of all of them and a partly applied one would admit or reject on
/// criteria the mapping does not carry.
fn parsed_filters(filters: &[String]) -> Option<Vec<Value>> {
    let mut parsed: Vec<Value> = Vec::with_capacity(filters.len());
    for pattern in filters {
        parsed.push(::service_common::event_pattern::parse(pattern).ok()?);
    }
    Some(parsed)
}

/// One record as the FILTER sees it: the record the function receives, with a `body` that
/// holds a JSON object or array read as that JSON.
///
/// The function always receives `body` as the string the queue holds — a message body "can
/// be any string" — but a filter names the fields INSIDE a structured body ("if your filter
/// criteria expects body to be in a valid JSON format"), so for matching a body that is a
/// JSON object or array is read as that document. A body that is anything else, a bare
/// scalar included, stays the string it is.
fn filter_event(record: &Value) -> Value {
    let mut event = record.clone();
    let parsed = record
        .get("body")
        .and_then(Value::as_str)
        .and_then(|body| ::serde_json::from_str::<Value>(body).ok())
        .filter(|body| body.is_object() || body.is_array());
    if let Some(body) = parsed {
        event["body"] = body;
    }
    event
}

/// The event a batch is handed over as: `Records`, holding the record of each message the
/// filter admitted, in the order the queue handed them over.
fn queue_event(invoking: &[String], polled: &[PolledMessage]) -> Value {
    let records: Vec<Value> = polled
        .iter()
        .filter(|message| invoking.contains(&message.message_id))
        .map(|message| message.record.clone())
        .collect();
    json!({ "Records": records })
}

/// One stored message as the record of the event Lambda hands a function for a queue
/// source: the message's own id, receipt handle, body and MD5 digest, the system attributes
/// the queue records for it, its message attributes, and which source it came from.
fn queue_record(
    message: &sqs::state::StoredMessage,
    receipt_handle: &str,
    queue_arn: &str,
    region: &str,
) -> Value {
    let mut attributes: BTreeMap<String, String> = BTreeMap::new();
    if !message.sender_id.is_empty() {
        attributes.insert("SenderId".to_string(), message.sender_id.clone());
    }
    attributes.insert(
        "SentTimestamp".to_string(),
        sqs::state::epoch_millis(message.sent_at),
    );
    attributes.insert(
        "ApproximateReceiveCount".to_string(),
        message.receive_count.to_string(),
    );
    if let Some(first) = message.first_received_at {
        attributes.insert(
            "ApproximateFirstReceiveTimestamp".to_string(),
            sqs::state::epoch_millis(first),
        );
    }
    if let Some(group_id) = &message.message_group_id {
        attributes.insert("MessageGroupId".to_string(), group_id.clone());
    }
    if let Some(deduplication_id) = &message.message_deduplication_id {
        attributes.insert(
            "MessageDeduplicationId".to_string(),
            deduplication_id.clone(),
        );
    }
    if let Some(sequence_number) = &message.sequence_number {
        attributes.insert("SequenceNumber".to_string(), sequence_number.clone());
    }
    let mut message_attributes: BTreeMap<String, Value> = BTreeMap::new();
    for (name, attribute) in &message.message_attributes {
        let mut rendered = json!({ "dataType": attribute.data_type.clone() });
        if let Some(text) = attribute.string_value.clone() {
            rendered["stringValue"] = Value::String(text);
        }
        if let Some(bytes) = attribute.binary_value.as_ref() {
            // A binary attribute travels as base64 in a JSON event, the way the record
            // Lambda hands a function reports one.
            rendered["binaryValue"] = Value::String(::base64::Engine::encode(
                &::base64::engine::general_purpose::STANDARD,
                bytes,
            ));
        }
        message_attributes.insert(name.clone(), rendered);
    }
    json!({
        "messageId": message.message_id.clone(),
        "receiptHandle": receipt_handle,
        "body": message.body.clone(),
        "attributes": attributes,
        "messageAttributes": message_attributes,
        "md5OfBody": message.md5_of_body.clone(),
        "eventSource": SQS_EVENT_SOURCE,
        "eventSourceARN": queue_arn,
        "awsRegion": region,
    })
}

/// Hand one batch to the function, synchronously, as a queue event source's invocation is.
async fn invoke_function(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    session: &mut PollSession,
    target: &PollTarget,
    payload: &Value,
) -> Result<(), String> {
    let answer = session
        .clients
        .lambda(callback, &target.region)
        .invoke()
        .function_name(&target.function_arn)
        .payload(::aws_sdk_lambda::primitives::Blob::new(
            payload.to_string().into_bytes(),
        ))
        .send()
        .await
        .map_err(|error| {
            format!(
                "{} could not be invoked ({})",
                target.function_arn,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    // "If present, indicates that an error occurred during function execution"
    // (`InvocationResponse$FunctionError`): the batch was not processed, so nothing of it is
    // deleted.
    if let Some(failure) = answer.function_error.as_deref() {
        return Err(format!(
            "{} answered a {failure} error for the batch",
            target.function_arn
        ));
    }
    Ok(())
}

/// Delete the messages `message_ids` names from `target`'s queue.
fn delete_messages(
    queues: &::engine::ServiceState<sqs::state::ComAmazonawsSqsAmazonsqsState>,
    target: &PollTarget,
    message_ids: &[String],
) {
    if message_ids.is_empty() {
        return;
    }
    let mut store = queues.lock();
    let Some(queue) = store.queue_by_arn_mut(&target.queue_arn) else {
        return;
    };
    for message_id in message_ids {
        queue.remove_message(message_id);
    }
}

/// Record what one processing attempt came to on the mapping itself.
///
/// The record is re-read under the lock and only written when it still names the queue and
/// the function this batch was polled for, so a mapping re-targeted while the batch was in
/// flight does not report an attempt made under a configuration it no longer carries.
fn record_attempt(
    mappings: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    target: &PollTarget,
    result: &str,
) {
    let mut store = mappings.lock();
    let Some(mapping) = store.event_source_mappings.get_mut(&target.uuid) else {
        return;
    };
    if mapping.event_source_arn.as_deref() != Some(target.queue_arn.as_str())
        || mapping.function_arn != target.function_arn
    {
        return;
    }
    mapping.last_processing_result = Some(result.to_string());
}

/// Whether the mapping `uuid` names may publish a 0-valued poll count at `at_secs` — true
/// once per minute, which is the resolution such a series is aggregated at — and claim that
/// minute when it may.
fn claim_idle_minute(session: &mut PollSession, uuid: &str, at_secs: f64) -> bool {
    let minute = minute_of(at_secs);
    if session.idle_minute.get(uuid) == Some(&minute) {
        return false;
    }
    session.idle_minute.insert(uuid.to_string(), minute);
    true
}

/// The minute `at_secs` (epoch seconds) falls in.
fn minute_of(at_secs: f64) -> i64 {
    (at_secs / SECONDS_PER_MINUTE).trunc() as i64
}

/// `at` as epoch seconds, which is how a CloudWatch datapoint carries its instant.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

/// A count of messages as a metric carries it.
fn count_of(messages: usize) -> u32 {
    u32::try_from(messages).unwrap_or(u32::MAX)
}

/// The queue of every mapping that is polling at this instant.
///
/// A mapping polls when it reports `Enabled` — "when true, the event source mapping is
/// active. When false, Lambda pauses polling and invocation" — its event source is an Amazon
/// SQS queue, and the function it invokes is one this store holds in [`STATE_ACTIVE`]:
/// "Lambda reads items from the event source and invokes the function", so a mapping whose
/// function cannot be invoked yet takes nothing out of the queue.
///
/// Read it only after [`ComAmazonawsLambdaAwsgirapiserviceState::materialize_all`], so a
/// mapping the clock has already moved into `Enabled` is polled and one it has removed is
/// not.
fn polling_targets(store: &ComAmazonawsLambdaAwsgirapiserviceState) -> Vec<PollTarget> {
    store
        .event_source_mappings
        .values()
        .filter(|mapping| {
            mapping.state == EVENT_SOURCE_MAPPING_STATE_ENABLED
                && !mapping.is_deleting()
                && store
                    .functions
                    .get(&mapping.function_name)
                    .is_some_and(|function| function.state == STATE_ACTIVE)
        })
        .filter_map(|mapping| {
            let queue_arn = queue_event_source(mapping.event_source_arn.as_deref())?;
            let region = ::service_common::arn::parse(queue_arn)?.region.to_string();
            Some(PollTarget {
                uuid: mapping.uuid.clone(),
                queue_arn: queue_arn.to_string(),
                region,
                function_arn: mapping.function_arn.clone(),
                // "Amazon Simple Queue Service – Default 10. For standard queues the max is
                // 10,000": the size is the batch's ceiling, and a mapping always pulls at
                // least one record.
                batch_size: usize::try_from(mapping.batch_size)
                    .unwrap_or(1)
                    .clamp(1, MAX_MESSAGES_PER_BATCH),
                filters: mapping
                    .filter_criteria
                    .as_ref()
                    .and_then(|criteria| criteria.filters.as_ref())
                    .map(|filters| {
                        filters
                            .iter()
                            .filter_map(|filter| filter.pattern.clone())
                            .collect()
                    })
                    .unwrap_or_default(),
                meters_event_counts: super::metrics::publishes_event_counts(
                    mapping.metrics_config.as_ref(),
                ),
            })
        })
        .collect()
}

/// `event_source_arn` when it names an Amazon SQS queue — "Amazon Simple Queue Service –
/// The ARN of the queue" — and `None` for every other event source, which no poller here
/// reads.
///
/// The value is stored request text, so it goes through the ONE shared ARN parser rather
/// than a second ad-hoc reader.
fn queue_event_source(event_source_arn: Option<&str>) -> Option<&str> {
    let arn = event_source_arn?;
    let parsed = ::service_common::arn::parse(arn)?;
    (parsed.vendor == EVENT_SOURCE_VENDOR_QUEUE && super::names_event_source(&parsed))
        .then_some(arn)
}

/// Claim the single poll loop for this store: the claim's GENERATION when the caller may
/// start one, `None` when one already runs.
fn claim_poller(store: &mut ComAmazonawsLambdaAwsgirapiserviceState) -> Option<u64> {
    if store.sqs_poller_running {
        return None;
    }
    store.sqs_poller_running = true;
    store.sqs_poller_generation = store.sqs_poller_generation.wrapping_add(1);
    Some(store.sqs_poller_generation)
}

/// Release the claim `generation` took — and ONLY that one, so a loop standing down cannot
/// clear a claim a loop started since is holding.
fn release_poller(store: &mut ComAmazonawsLambdaAwsgirapiserviceState, generation: u64) {
    if store.sqs_poller_generation == generation {
        store.sqs_poller_running = false;
    }
}

/// Keep the running loop's task handle, aborting the one it replaces.
///
/// Only when `generation` is still the CURRENT claim. A loop can end before its starter gets
/// here (it releases its own claim as soon as no mapping polls), and a call that claimed the
/// loop since then has stored a handle of its own: replacing that one would abort a live
/// loop and leave its claim held for ever. So a stale starter installs nothing and aborts
/// nothing — its own task has already released its claim, so it stands down on its own.
fn set_poller(
    store: &mut ComAmazonawsLambdaAwsgirapiserviceState,
    generation: u64,
    handle: ::tokio::task::JoinHandle<()>,
) {
    if store.sqs_poller_generation != generation {
        return;
    }
    if let Some(previous) = store.sqs_poller.replace(handle) {
        previous.abort();
    }
}

/// Whether any mapping of this store still wants polling — and, when none does, RELEASE the
/// claim `generation` holds.
///
/// Observing "nothing polls" and standing down happen in ONE critical section, which is what
/// makes the hand-over race-free: a create holds this same lock to store its mapping BEFORE
/// it tries to claim the loop, so either this call sees that mapping and the running loop
/// keeps it, or the create's claim finds the flag cleared and starts a loop of its own.
///
/// A mapping still SETTLING into `Enabled` counts: the state it settles into is its
/// `Enabled` setting, and the clock owes that move, so the loop waits for it rather than
/// standing down between the create and the move it already knows about.
fn polling_wanted_or_release(
    store: &mut ComAmazonawsLambdaAwsgirapiserviceState,
    generation: u64,
) -> bool {
    let wanted = store.event_source_mappings.values().any(|mapping| {
        mapping.enabled
            && !mapping.is_deleting()
            && queue_event_source(mapping.event_source_arn.as_deref()).is_some()
    });
    if !wanted {
        release_poller(store, generation);
    }
    wanted
}

#[cfg(test)]
mod tests {
    use ::serde_json::json;

    use super::{filter_event, minute_of, queue_event, PolledMessage};

    /// One polled message carrying `body`, under the id `id`.
    fn polled(id: &str, body: &str) -> PolledMessage {
        PolledMessage {
            message_id: id.to_string(),
            record: json!({ "messageId": id, "body": body }),
        }
    }

    #[test]
    fn a_json_body_is_read_as_json_for_the_filter_and_left_a_string_for_the_function() {
        // The rows differ along what the body IS: an object, an array, a bare scalar that
        // is still valid JSON, and text that is not JSON at all. Only the first two are the
        // "valid JSON format" a filter names fields inside.
        let rows = [
            ("an object", r#"{"kind":"keep"}"#, json!({ "kind": "keep" })),
            ("an array", r#"[1,2]"#, json!([1, 2])),
            ("a bare number", "17", json!("17")),
            ("plain text", "keep", json!("keep")),
        ];
        for (case, body, expected) in rows {
            let message = polled("m-1", body);
            let event = filter_event(&message.record);
            assert_eq!(
                event["body"], expected,
                "case {case:?}: the filter must see {expected:?}, got {:?}",
                event["body"]
            );
            assert_eq!(
                message.record["body"],
                json!(body),
                "case {case:?}: the function must still see the body as the string it is, \
                 got {:?}",
                message.record["body"]
            );
        }
    }

    #[test]
    fn the_event_carries_only_the_records_the_filter_admitted() {
        let polled_messages = vec![polled("m-1", "a"), polled("m-2", "b"), polled("m-3", "c")];
        let rows: [(&str, Vec<String>, Vec<&str>); 3] = [
            ("none admitted", Vec::new(), Vec::new()),
            (
                "the middle one admitted",
                vec!["m-2".to_string()],
                vec!["m-2"],
            ),
            (
                "all admitted",
                vec!["m-1".to_string(), "m-2".to_string(), "m-3".to_string()],
                vec!["m-1", "m-2", "m-3"],
            ),
        ];
        for (case, invoking, expected) in rows {
            let event = queue_event(&invoking, &polled_messages);
            let ids: Vec<String> = event["Records"]
                .as_array()
                .map(|records| {
                    records
                        .iter()
                        .map(|record| record["messageId"].as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                ids, expected,
                "case {case:?}: expected the records {expected:?}, got {ids:?}"
            );
        }
    }

    #[test]
    fn a_minute_is_the_window_an_idle_poll_count_is_published_once_in() {
        let rows = [
            ("the epoch", 0.0, 0),
            ("the first minute", 59.9, 0),
            ("the next minute", 60.0, 1),
            ("a later instant", 1_761_000_000.0, 29_350_000),
        ];
        for (case, at_secs, expected) in rows {
            let minute = minute_of(at_secs);
            assert_eq!(
                minute, expected,
                "case {case:?}: expected minute {expected}, got {minute}"
            );
        }
    }
}
