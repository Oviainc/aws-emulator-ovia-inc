// SPDX-License-Identifier: CC0-1.0

//! The pipes DATA PLANE: what a pipe does with no client action at all.
//!
//! A pipe's control plane (the record, its ARN, its lifecycle states) lives in
//! [`crate::state`]. This module is the other half: once a pipe's `CurrentState` is
//! `RUNNING`, EventBridge Pipes itself polls the source, hands what it polled to the
//! enrichment, and invokes the target — no caller asks it to. The three stages and the
//! rules they follow:
//!
//! * SOURCE. The queue is polled for up to `BatchSize` records, and a poll that has not
//!   filled a batch keeps collecting for up to `MaximumBatchingWindowInSeconds` —
//!   "the maximum number of records to include in each batch" and "the maximum length of
//!   a time to wait for events" (`PipeSourceSqsQueueParameters`). Those two bound the
//!   POLL; they say nothing about how many calls the target stage makes.
//! * ENRICHMENT. The records polled together are handed to the enrichment as ONE JSON
//!   array of record envelopes, and what the enrichment RETURNS — not the source batch —
//!   is what the target stage delivers. So an enrichment that collapses a batch into one
//!   aggregate produces fewer target deliveries than there were source records, one that
//!   returns several events produces more, and one that returns nothing produces none.
//! * TARGET. Each event of the array handed to the target stage is delivered on its own:
//!   an Amazon SQS queue's `SendMessage` takes ONE payload per call, and
//!   `PipeTargetSqsQueueParameters` carries only `MessageGroupId` and
//!   `MessageDeduplicationId` — no batch member — so a batch of N events becomes N
//!   messages of one event each, never one message holding an array.
//!
//! A source record is deleted only once every event derived from it has been delivered,
//! so a target the pipe could not reach leaves the record on the queue to reappear when
//! its visibility timeout lapses.
//!
//! The poller is started by the operations that make a pipe RUNNING (`CreatePipe`,
//! `StartPipe`, `UpdatePipe`) and ends itself as soon as the pipe's `CurrentState` is no
//! longer one a running pipe rests in — so a `STOPPED` pipe consumes nothing, its
//! backlog stays visible and undeleted, and `StartPipe` resumes consumption of
//! everything that accumulated while it was stopped. At most one poller runs per pipe
//! ([`ComAmazonawsPipesPipesState::claim_poller`]).
//!
//! Every call this module makes goes to the emulator it is running inside, over the
//! in-process callback ([`smithy_server_runtime::EmbeddedEmulator`]): the pipe reaches
//! the sqs, lambda and states surfaces of this same emulator and nothing else.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ::serde_json::{json, Value};

use super::{pipe_state, ComAmazonawsPipesPipesState, PipeRecord};

/// How long the poller waits before looking at an idle source again. Short enough that
/// a record sent to the source is moved promptly, long enough not to spin.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long the poller waits after a stage failed before trying the batch again. It is
/// multiplied by the number of consecutive failures, up to [`MAX_RETRY_BACKOFF`].
const RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// The longest the poller waits between two attempts at a batch it keeps failing to
/// move.
///
/// A pipe whose source or target cannot be reached keeps trying — that is what the
/// service does, and it is what lets a pipe recover when the resource comes back — but
/// it does so at this interval rather than at the rate a healthy pipe polls.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(5);

/// How often a pipe that keeps failing says so: the first failure, and every this many
/// after it, so one broken pipe cannot fill the log.
const FAILURE_LOG_INTERVAL: u32 = 20;

/// "The maximum number of messages to return. ... Valid values: 1 to 10."
/// (`ReceiveMessageRequest$MaxNumberOfMessages`), so a batch larger than this is
/// collected over several receives.
const MAX_RECEIVE_PER_CALL: i32 = 10;

/// The most records this emulator holds in ONE batch, whatever `BatchSize` the pipe
/// configures.
///
/// This is the EMULATOR's bound, not an AWS number (`BatchSize` itself reaches 10000):
/// a batch is held in memory, and the enrichment payload is built from the whole of it,
/// so the count a caller chooses is clamped before anything is collected. A pipe
/// configured above it polls this many records per batch instead, which is a smaller
/// batch and never a dropped record.
const MAX_RECORDS_PER_BATCH: usize = 1_000;

/// The `BatchSize` a pipe whose source parameters name none polls with.
///
/// The model declares no `@default` on the member (only its `@range`), so this is the
/// default EventBridge Pipes documents for an Amazon SQS source — ten records per batch,
/// the same default the Amazon SQS event-source polling of a Lambda function has. A pipe
/// that wants one record per batch says `BatchSize: 1`.
const DEFAULT_BATCH_SIZE: usize = 10;

/// `BatchSize`'s own `@range`, from `PipeSourceSqsQueueParameters` in
/// `smithy-model/com-amazonaws-pipes-pipes.json`.
const BATCH_SIZE_RANGE: std::ops::RangeInclusive<usize> = 1..=10_000;

/// `MaximumBatchingWindowInSeconds`' own `@range`, from the same shape.
const BATCHING_WINDOW_RANGE: std::ops::RangeInclusive<i32> = 0..=300;

/// The `eventSource` an Amazon SQS record envelope reports.
const SQS_EVENT_SOURCE: &str = "aws:sqs";

/// The ARN service namespaces the stages below address: an SQS queue, a Lambda function
/// and a Step Functions state machine (`states`, the namespace Step Functions signs and
/// publishes its endpoint under).
const SQS_NAMESPACE: &str = "sqs";
const LAMBDA_NAMESPACE: &str = "lambda";
const STATES_NAMESPACE: &str = "states";

/// The resource types the enrichment ARNs name within those namespaces.
const FUNCTION_RESOURCE: &str = "function";
const STATE_MACHINE_RESOURCE: &str = "stateMachine";

/// `ReceiveMessage`'s selector for every attribute: "All".
const ALL_ATTRIBUTES: &str = "All";

/// Start the in-process poller for the pipe named `pipe_name`, if one is not already
/// running for it.
///
/// Called by an operation that has made the pipe RUNNING, AFTER it has released the
/// store's lock. It changes nothing a caller is told: the poller observes the pipe's own
/// `CurrentState` and does nothing until the clock has moved the pipe into `RUNNING`.
/// With no async runtime to spawn onto there is no poller and nothing is polled.
///
/// At most ONE task runs per pipe, and the store holds at most [`super::MAX_PIPES`]
/// pipes, so the tasks this can ever spawn are bounded by that count. A task is ended by
/// the lifecycle call that stops or deletes its pipe — it reads the pipe's state every
/// pass and returns as soon as the pipe is not RUNNING — so none of them outlives the
/// pipe it polls, and its claim is released however it ends (see [`PollerClaim`]).
pub fn drive_pipe<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &::std::sync::Arc<Eng>,
    pipe_name: &str,
) {
    let Some(state) = super::service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    if !state.lock().claim_poller(pipe_name) {
        return;
    }
    let callback = ctx.callback().clone();
    let claim = PollerClaim {
        state: ::engine::ServiceState::clone(&state),
        pipe: pipe_name.to_string(),
        held: true,
    };
    runtime.spawn(async move {
        poll_pipe(&callback, claim).await;
    });
}

/// The claim of the one poller of a pipe, held for as long as that poller runs.
///
/// It is released by [`Self::release`] inside the critical section that decided the
/// poller is done, so a lifecycle call that arrives afterwards starts a new poller. The
/// `Drop` is the backstop for every OTHER way the task can end — the runtime shutting
/// down, or a panic inside the poller — because a claim left behind would stop this pipe
/// from ever being polled again.
struct PollerClaim {
    state: ::engine::ServiceState<ComAmazonawsPipesPipesState>,
    pipe: String,
    /// Whether this claim is still the live one. Set to `false` by [`Self::release`], so
    /// the `Drop` cannot release a claim a LATER poller has since taken.
    held: bool,
}

impl PollerClaim {
    /// Release the claim through a store lock the caller already holds.
    fn release(&mut self, store: &mut ComAmazonawsPipesPipesState) {
        if self.held {
            store.release_poller(&self.pipe);
            self.held = false;
        }
    }
}

impl Drop for PollerClaim {
    fn drop(&mut self) {
        if self.held {
            let mut store = self.state.lock();
            store.release_poller(&self.pipe);
            self.held = false;
        }
    }
}

/// What the poller may do on one pass, decided under the store's lock.
enum Pass {
    /// The pipe is RUNNING: move one batch under this configuration. Boxed because it is
    /// far larger than the other answers, which are what most passes are.
    Run(Box<SqsSourcedPipe>),
    /// The pipe is mid-transition (`CREATING`, `STARTING`, `UPDATING`): nothing is
    /// consumed until it rests in `RUNNING`.
    Wait,
    /// The pipe is RUNNING but its configuration is not one this data plane serves, and
    /// why. Nothing is polled and the poller ends, so no record is consumed by a stage
    /// that would deliver it wrongly.
    Unserved(String),
    /// The pipe is no longer one this poller runs: it rests `STOPPED`, is stopping or
    /// being deleted, or the record is gone. The claim is already released.
    Stop,
}

/// Poll one pipe until it is no longer RUNNING.
///
/// The store's lock is taken for the decision of each pass and released before any call
/// leaves this process, so a request never waits on a delivery.
async fn poll_pipe(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    mut claim: PollerClaim,
) {
    let pipe_name = claim.pipe.clone();
    // A handle of its own, so the store can be locked while the claim is borrowed.
    let state = ::engine::ServiceState::clone(&claim.state);
    let mut clients = Clients::default();
    let mut failures = 0u32;
    loop {
        let pass = {
            let mut store = state.lock();
            // The state moves the clock has ALREADY made due, so a pipe whose
            // `STARTING` span has lapsed is seen as the `RUNNING` pipe it became.
            store.settle_due_transitions();
            next_pass(&mut store, &mut claim)
        };
        match pass {
            Pass::Stop => return,
            Pass::Wait => ::tokio::time::sleep(POLL_INTERVAL).await,
            Pass::Unserved(detail) => {
                ::tracing::warn!(
                    pipe = %pipe_name,
                    detail = %detail,
                    "this emulator's pipes data plane does not serve this pipe's \
                     configuration, so its source is left untouched"
                );
                let mut store = state.lock();
                claim.release(&mut store);
                return;
            }
            Pass::Run(run) => match move_one_batch(callback, &mut clients, &run).await {
                Batch::Moved => failures = 0,
                Batch::Idle => {
                    failures = 0;
                    ::tokio::time::sleep(POLL_INTERVAL).await;
                }
                Batch::Failed(detail) => {
                    failures = failures.saturating_add(1);
                    if failures == 1 || failures.is_multiple_of(FAILURE_LOG_INTERVAL) {
                        ::tracing::warn!(
                            pipe = %pipe_name,
                            attempt = failures,
                            detail = %detail,
                            "a pipe could not move a batch from its source to its \
                             target; the records stay on the source and it tries again"
                        );
                    }
                    ::tokio::time::sleep(retry_backoff(failures)).await;
                }
            },
        }
    }
}

/// How long to wait after `failures` consecutive failed attempts at one batch: it grows
/// with them, capped at [`MAX_RETRY_BACKOFF`].
fn retry_backoff(failures: u32) -> Duration {
    RETRY_BACKOFF
        .saturating_mul(failures.max(1))
        .min(MAX_RETRY_BACKOFF)
}

/// What the poller does next for the pipe it claims, releasing its claim when the answer
/// is [`Pass::Stop`] — the release and the state it was decided from are read in ONE
/// critical section, so a `StartPipe` that has already moved the pipe back towards
/// `RUNNING` either keeps this poller or starts a new one, never neither.
fn next_pass(store: &mut ComAmazonawsPipesPipesState, claim: &mut PollerClaim) -> Pass {
    let pipe_name = claim.pipe.clone();
    let Some(pipe) = store.pipes.get(&pipe_name) else {
        claim.release(store);
        return Pass::Stop;
    };
    if pipe.current_state == pipe_state::RUNNING {
        return match resolve_run(pipe) {
            Ok(run) => Pass::Run(Box::new(run)),
            // A configuration this data plane cannot serve is reported by the pass
            // itself, so the one log line is written outside the lock.
            Err(detail) => Pass::Unserved(detail),
        };
    }
    // The states a pipe is in while a move it is already making lands. A pipe that is
    // on its way to RUNNING keeps its poller; everything else ends it.
    let towards_running = matches!(
        pipe.current_state.as_str(),
        pipe_state::CREATING | pipe_state::STARTING | pipe_state::UPDATING
    ) && pipe.desired_state == super::requested_state::RUNNING;
    if towards_running {
        return Pass::Wait;
    }
    claim.release(store);
    Pass::Stop
}

/// What one pass of the poller came to.
enum Batch {
    /// A batch was polled and every event it produced was delivered.
    Moved,
    /// The source held nothing.
    Idle,
    /// A stage failed; the source records are left for the next pass (and reappear on
    /// the queue when their visibility timeout lapses).
    Failed(String),
}

/// Move one batch: poll the source, hand it to the enrichment, deliver what came back to
/// the target, and delete the source records that were delivered.
async fn move_one_batch(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &SqsSourcedPipe,
) -> Batch {
    let polled = match collect_batch(callback, clients, run).await {
        Ok(polled) => polled,
        Err(detail) => return Batch::Failed(detail),
    };
    if polled.is_empty() {
        return Batch::Idle;
    }

    // The WHOLE polled batch, as one JSON array of record envelopes.
    let batch: Vec<Value> = polled
        .iter()
        .map(|message| record_envelope(message, &run.source_arn, &run.source_region))
        .collect();

    // What the TARGET stage delivers: the enrichment's answer when the pipe has one,
    // the polled batch itself when it has none.
    let events = match &run.enrichment {
        None => batch,
        Some(enrichment) => match invoke_enrichment(callback, clients, enrichment, &batch).await {
            Ok(events) => events,
            Err(detail) => return Batch::Failed(detail),
        },
    };

    // One target call per event, never one call holding the array.
    for event in &events {
        if let Err(detail) = deliver_event(callback, clients, &run.target, event).await {
            return Batch::Failed(detail);
        }
    }

    // Delivered: the source records may go. A delete that fails only means the record
    // reappears and is delivered again, which is the at-least-once the service promises.
    let queue = run.source_queue.clone();
    for message in &polled {
        let Some(handle) = message.receipt_handle.as_deref() else {
            continue;
        };
        if let Err(error) = clients
            .sqs(callback, &run.source_region)
            .delete_message()
            .queue_url(&queue)
            .receipt_handle(handle)
            .send()
            .await
        {
            ::tracing::warn!(
                queue = %queue,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a delivered pipe source record could not be deleted"
            );
        }
    }
    Batch::Moved
}

/// Poll the source for up to `BatchSize` records, collecting for up to the batching
/// window once the first record has arrived.
///
/// The window is what lets a batch span several receives: "the maximum length of a time
/// to wait for events". It starts when the poll finds its first record, so an idle
/// source is not waited on — this pass simply reports nothing and the next one looks
/// again.
async fn collect_batch(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &SqsSourcedPipe,
) -> Result<Vec<::aws_sdk_sqs::types::Message>, String> {
    let mut collected = receive(callback, clients, run, run.batch_size).await?;
    if collected.is_empty() {
        return Ok(collected);
    }
    let deadline = Instant::now() + run.batching_window;
    while collected.len() < run.batch_size && Instant::now() < deadline {
        let wanted = run.batch_size - collected.len();
        let more = receive(callback, clients, run, wanted).await?;
        if more.is_empty() {
            ::tokio::time::sleep(POLL_INTERVAL).await;
        } else {
            collected.extend(more);
        }
    }
    Ok(collected)
}

/// One `ReceiveMessage` for up to `wanted` records, with every attribute a record
/// envelope reports.
async fn receive(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &SqsSourcedPipe,
    wanted: usize,
) -> Result<Vec<::aws_sdk_sqs::types::Message>, String> {
    let asked = i32::try_from(wanted)
        .unwrap_or(MAX_RECEIVE_PER_CALL)
        .clamp(1, MAX_RECEIVE_PER_CALL);
    let answer = clients
        .sqs(callback, &run.source_region)
        .receive_message()
        .queue_url(&run.source_queue)
        .max_number_of_messages(asked)
        .message_system_attribute_names(::aws_sdk_sqs::types::MessageSystemAttributeName::All)
        .message_attribute_names(ALL_ATTRIBUTES)
        .send()
        .await
        .map_err(|error| {
            format!(
                "the source queue {} could not be received from ({})",
                run.source_queue,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    Ok(answer.messages.unwrap_or_default())
}

/// One polled Amazon SQS message as the record envelope a pipe hands to its enrichment
/// and its target: the message's own id, receipt handle, body and MD5, its system and
/// message attributes, and which source it came from.
fn record_envelope(
    message: &::aws_sdk_sqs::types::Message,
    source_arn: &str,
    region: &str,
) -> Value {
    let mut attributes: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in message.attributes.iter().flatten() {
        attributes.insert(name.as_str().to_string(), value.clone());
    }
    let mut message_attributes: BTreeMap<String, Value> = BTreeMap::new();
    for (name, attribute) in message.message_attributes.iter().flatten() {
        let mut rendered = json!({ "dataType": attribute.data_type.clone() });
        if let Some(text) = attribute.string_value.clone() {
            rendered["stringValue"] = Value::String(text);
        }
        if let Some(bytes) = attribute.binary_value.as_ref() {
            // A binary attribute travels as base64 in a JSON envelope, the way the
            // record envelope reports one.
            rendered["binaryValue"] = Value::String(
                ::base64::Engine::encode(&::base64::engine::general_purpose::STANDARD, bytes.as_ref()),
            );
        }
        message_attributes.insert(name.clone(), rendered);
    }
    json!({
        "messageId": message.message_id.clone().unwrap_or_default(),
        "receiptHandle": message.receipt_handle.clone().unwrap_or_default(),
        "body": message.body.clone().unwrap_or_default(),
        "attributes": attributes,
        "messageAttributes": message_attributes,
        "md5OfBody": message.md5_of_body.clone().unwrap_or_default(),
        "eventSource": SQS_EVENT_SOURCE,
        "eventSourceARN": source_arn,
        "awsRegion": region,
    })
}

/// Hand the whole polled batch to the enrichment as ONE array, and read the events it
/// returned.
///
/// What comes back drives the target stage: an array is that many events, a single JSON
/// value is one event, and nothing at all is NO event — which suppresses the target
/// invocation for this batch.
async fn invoke_enrichment(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    enrichment: &Enrichment,
    batch: &[Value],
) -> Result<Vec<Value>, String> {
    let payload = Value::Array(batch.to_vec()).to_string();
    match enrichment {
        Enrichment::LambdaFunction { arn, region } => {
            let answer = clients
                .lambda(callback, region)
                .invoke()
                .function_name(arn)
                .payload(::aws_sdk_lambda::primitives::Blob::new(payload.into_bytes()))
                .send()
                .await
                .map_err(|error| {
                    format!(
                        "the enrichment {arn} could not be invoked ({})",
                        ::aws_smithy_types::error::display::DisplayErrorContext(&error)
                    )
                })?;
            // "If present, indicates that an error occurred during function execution"
            // (`InvocationResponse$FunctionError`): the batch is not enriched, so
            // nothing is delivered and the records stay on the source.
            if let Some(failure) = answer.function_error.as_deref() {
                return Err(format!("the enrichment {arn} answered a {failure} error"));
            }
            let returned = answer
                .payload
                .map(::aws_sdk_lambda::primitives::Blob::into_inner)
                .unwrap_or_default();
            enrichment_events(&returned, arn)
        }
        Enrichment::ExpressWorkflow { arn, region } => {
            let answer = clients
                .sfn(callback, region)
                .start_sync_execution()
                .state_machine_arn(arn)
                .input(payload)
                .send()
                .await
                .map_err(|error| {
                    format!(
                        "the enrichment {arn} could not be run ({})",
                        ::aws_smithy_types::error::display::DisplayErrorContext(&error)
                    )
                })?;
            if answer.status != ::aws_sdk_sfn::types::SyncExecutionStatus::Succeeded {
                return Err(format!(
                    "the enrichment {arn} ended {}",
                    answer.status.as_str()
                ));
            }
            let returned = answer.output.unwrap_or_default();
            enrichment_events(returned.as_bytes(), arn)
        }
    }
}

/// The events an enrichment's answer carries: the elements of a returned array, the one
/// value a returned object or scalar is, or none when it returned nothing.
fn enrichment_events(returned: &[u8], arn: &str) -> Result<Vec<Value>, String> {
    let trimmed: &[u8] = {
        let text = returned;
        let start = text.iter().position(|byte| !byte.is_ascii_whitespace());
        match start {
            None => &[],
            Some(start) => &text[start..],
        }
    };
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = ::serde_json::from_slice(trimmed)
        .map_err(|error| format!("the enrichment {arn} answered a body that is not JSON ({error})"))?;
    Ok(match value {
        Value::Null => Vec::new(),
        Value::Array(events) => events,
        single => vec![single],
    })
}

/// Deliver ONE event to the target.
async fn deliver_event(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    target: &Target,
    event: &Value,
) -> Result<(), String> {
    match target {
        Target::Queue {
            url,
            region,
            message_group_id,
            message_deduplication_id,
        } => {
            let mut request = clients
                .sqs(callback, region)
                .send_message()
                .queue_url(url)
                .message_body(target_body(event));
            if let Some(group) = message_group_id {
                request = request.message_group_id(group);
            }
            if let Some(deduplication) = message_deduplication_id {
                request = request.message_deduplication_id(deduplication);
            }
            request.send().await.map_err(|error| {
                format!(
                    "the target queue {url} did not take the event ({})",
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error)
                )
            })?;
            Ok(())
        }
    }
}

/// One event as a target message body: the text of a string event, the JSON of any
/// other.
fn target_body(event: &Value) -> String {
    match event {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// An Amazon SQS-sourced pipe, as the stages above read it: what the poller moves for
/// one pipe, resolved from the stored record.
struct SqsSourcedPipe {
    /// The queue url the source ARN addresses.
    source_queue: String,
    /// The `Source` itself, which every record envelope reports as its event source.
    source_arn: String,
    /// The Region the source ARN names — the one its records are polled in and report.
    source_region: String,
    /// `BatchSize`: the most records one batch holds.
    batch_size: usize,
    /// `MaximumBatchingWindowInSeconds`, as a span.
    batching_window: Duration,
    /// The enrichment the batch is handed to, when the pipe has one.
    enrichment: Option<Enrichment>,
    target: Target,
}

/// The enrichment a batch is handed to, by the resource its ARN names.
enum Enrichment {
    /// A Lambda function, invoked with the batch as its payload.
    LambdaFunction { arn: String, region: String },
    /// A Step Functions EXPRESS workflow, run synchronously with the batch as its
    /// input: "You can only use Express workflows as an enrichment", and a synchronous
    /// run is what answers the enriched events.
    ExpressWorkflow { arn: String, region: String },
}

/// The target an event is delivered to, by the resource its ARN names.
enum Target {
    /// An Amazon SQS queue: one `SendMessage` per event.
    Queue {
        url: String,
        region: String,
        message_group_id: Option<String>,
        message_deduplication_id: Option<String>,
    },
}

/// What this record asks the stages to do, or why this data plane does not serve it.
///
/// A configuration it cannot serve FAITHFULLY leaves the source untouched rather than
/// delivering something the pipe did not ask for: an input template that would reshape
/// the payload, a filter that would drop records, or a source or target of a kind no
/// stage here addresses.
fn resolve_run(pipe: &PipeRecord) -> Result<SqsSourcedPipe, String> {
    let source = ::service_common::arn::parse(&pipe.source)
        .ok_or_else(|| format!("its Source {} is not a resource ARN", pipe.source))?;
    if source.vendor != SQS_NAMESPACE {
        return Err(format!(
            "its Source names a {} resource, and only an Amazon SQS queue source is \
             polled by this emulator",
            source.vendor
        ));
    }
    let queue_name = source.resource;
    if queue_name.contains('/') || queue_name.contains(':') {
        return Err(format!("its Source {} names no queue", pipe.source));
    }
    let source_region = source.region.to_string();
    let source_queue = queue_url(&source_region, source.account, queue_name);

    let parameters = pipe.source_parameters.as_ref();
    if parameters.and_then(|p| p.filter_criteria.as_ref()).is_some() {
        return Err("it carries FilterCriteria, which this emulator does not evaluate yet"
            .to_string());
    }
    let sqs_parameters = parameters.and_then(|p| p.sqs_queue_parameters.as_ref());
    let batch_size = sqs_parameters
        .and_then(|p| p.batch_size)
        .and_then(|size| usize::try_from(size).ok())
        .unwrap_or(DEFAULT_BATCH_SIZE)
        .clamp(*BATCH_SIZE_RANGE.start(), *BATCH_SIZE_RANGE.end())
        // Clamped BEFORE anything is collected, so the records one batch holds in
        // memory are bounded by this emulator rather than by the request.
        .min(MAX_RECORDS_PER_BATCH);
    let window_seconds = sqs_parameters
        .and_then(|p| p.maximum_batching_window_in_seconds)
        .unwrap_or(0)
        .clamp(*BATCHING_WINDOW_RANGE.start(), *BATCHING_WINDOW_RANGE.end());
    let batching_window = Duration::from_secs(u64::try_from(window_seconds).unwrap_or_default());

    if pipe
        .enrichment_parameters
        .as_ref()
        .and_then(|parameters| parameters.input_template.as_deref())
        .is_some_and(|template| !template.is_empty())
    {
        return Err(
            "its EnrichmentParameters carry an InputTemplate, which this emulator does not \
             render yet"
                .to_string(),
        );
    }
    let enrichment = match pipe
        .enrichment
        .as_deref()
        .filter(|arn| !arn.is_empty())
    {
        None => None,
        Some(arn) => Some(resolve_enrichment(arn)?),
    };

    let target_parameters = pipe.target_parameters.as_ref();
    if target_parameters
        .and_then(|parameters| parameters.input_template.as_deref())
        .is_some_and(|template| !template.is_empty())
    {
        return Err(
            "its TargetParameters carry an InputTemplate, which this emulator does not render \
             yet"
                .to_string(),
        );
    }
    let target = resolve_target(
        &pipe.target,
        target_parameters.and_then(|parameters| parameters.sqs_queue_parameters.as_ref()),
    )?;

    Ok(SqsSourcedPipe {
        source_queue,
        source_arn: pipe.source.clone(),
        source_region,
        batch_size,
        batching_window,
        enrichment,
        target,
    })
}

/// The enrichment the `Enrichment` ARN names, or why this data plane does not invoke it.
fn resolve_enrichment(arn: &str) -> Result<Enrichment, String> {
    let parsed = ::service_common::arn::parse(arn)
        .ok_or_else(|| format!("its Enrichment {arn} is not a resource ARN"))?;
    let region = parsed.region.to_string();
    match (parsed.vendor, parsed.resource_type()) {
        (LAMBDA_NAMESPACE, FUNCTION_RESOURCE) => Ok(Enrichment::LambdaFunction {
            arn: arn.to_string(),
            region,
        }),
        (STATES_NAMESPACE, STATE_MACHINE_RESOURCE) => Ok(Enrichment::ExpressWorkflow {
            arn: arn.to_string(),
            region,
        }),
        _ => Err(format!(
            "its Enrichment {arn} names a resource this emulator's enrichment stage does not \
             invoke"
        )),
    }
}

/// The target the `Target` ARN names, or why this data plane does not deliver to it.
fn resolve_target(
    arn: &str,
    queue_parameters: Option<&crate::ops::types::PipeTargetSqsQueueParameters>,
) -> Result<Target, String> {
    let parsed = ::service_common::arn::parse(arn)
        .ok_or_else(|| format!("its Target {arn} is not a resource ARN"))?;
    if parsed.vendor != SQS_NAMESPACE {
        return Err(format!(
            "its Target names a {} resource, and only an Amazon SQS queue target is delivered \
             to by this emulator",
            parsed.vendor
        ));
    }
    let name = parsed.resource;
    if name.contains('/') || name.contains(':') {
        return Err(format!("its Target {arn} names no queue"));
    }
    Ok(Target::Queue {
        url: queue_url(parsed.region, parsed.account, name),
        region: parsed.region.to_string(),
        message_group_id: queue_parameters
            .and_then(|parameters| parameters.message_group_id.clone())
            .filter(|group| !group.is_empty()),
        message_deduplication_id: queue_parameters
            .and_then(|parameters| parameters.message_deduplication_id.clone())
            .filter(|token| !token.is_empty()),
    })
}

/// The url Amazon SQS addresses the queue named `name` by:
/// `<endpoint>/<accountId>/<QueueName>`, the form every SQS operation takes as its
/// `QueueUrl`. An ARN with no account field is this emulator's own account.
fn queue_url(region: &str, account: &str, name: &str) -> String {
    let account = if account.is_empty() {
        ::engine::emulator_account_id().to_string()
    } else {
        account.to_string()
    };
    format!("https://sqs.{region}.amazonaws.com/{account}/{name}")
}

/// The clients the stages reach the other services of this emulator through, built on
/// first use and then reused.
///
/// Each slot remembers the Region its client was built for, because a resource is
/// reached in the Region ITS OWN ARN names: a pipe whose source and target are in two
/// Regions rebuilds the client rather than sending the delivery to the source's Region.
#[derive(Default)]
struct Clients {
    sqs: Option<(String, ::aws_sdk_sqs::Client)>,
    lambda: Option<(String, ::aws_sdk_lambda::Client)>,
    sfn: Option<(String, ::aws_sdk_sfn::Client)>,
}

/// Build each client the same way: over the in-process callback into this same
/// emulator, against the called service's endpoint in `region`.
///
/// The endpoint url names the called service's public host so the request is signed and
/// routed as that service's, but the TRANSPORT is the emulator's own in-process
/// connector ([`smithy_server_runtime::EmbeddedEmulator::http_client`]): no socket is
/// opened and no name is resolved, so a queue, function or workflow a caller named is
/// reached only if this emulator serves it itself.
///
/// The transport is in-process, so each client disables what a network client needs and
/// this one cannot have: retries, timeouts, stalled-stream protection and identity
/// caching each need a sleep implementation the callback transport is not given.
macro_rules! callback_client {
    ($field:ident, $sdk:ident, $endpoint:literal) => {
        fn $field(
            &mut self,
            callback: &smithy_server_runtime::EmbeddedEmulator,
            region: &str,
        ) -> &::$sdk::Client {
            if self
                .$field
                .as_ref()
                .is_none_or(|(held, _)| held != region)
            {
                let config = ::$sdk::Config::builder()
                    .behavior_version(::$sdk::config::BehaviorVersion::latest())
                    .http_client(callback.http_client())
                    .credentials_provider(callback.aws_credentials_provider())
                    .region(::$sdk::config::Region::new(region.to_string()))
                    .endpoint_url(format!("https://{}.{region}.amazonaws.com", $endpoint))
                    .retry_config(::$sdk::config::retry::RetryConfig::disabled())
                    .timeout_config(::$sdk::config::timeout::TimeoutConfig::disabled())
                    .identity_cache(::$sdk::config::IdentityCache::no_cache())
                    .stalled_stream_protection(
                        ::$sdk::config::StalledStreamProtectionConfig::disabled(),
                    )
                    .build();
                self.$field = Some((region.to_string(), ::$sdk::Client::from_conf(config)));
            }
            &self
                .$field
                .as_ref()
                .expect("the client for this region was just built")
                .1
        }
    };
}

impl Clients {
    callback_client!(sqs, aws_sdk_sqs, "sqs");
    callback_client!(lambda, aws_sdk_lambda, "lambda");
    // Step Functions publishes its endpoint and signs under `states`.
    callback_client!(sfn, aws_sdk_sfn, "states");
}

#[cfg(test)]
mod tests {
    use super::{enrichment_events, target_body};
    use ::serde_json::{json, Value};

    /// RULE: what an enrichment RETURNS is what the target stage delivers — the
    /// elements of a returned array (however many), the one value a returned object or
    /// scalar is, and nothing at all for an answer that carries no value.
    ///
    /// The rows differ along the shape of the answer, which is what the rule
    /// generalises over: the empty answer, an empty array, one object, several objects,
    /// a scalar, and `null`.
    #[test]
    fn an_enrichment_answer_is_read_as_the_events_it_returned() {
        let rows: Vec<(&str, &str, usize)> = vec![
            ("no-answer", "", 0),
            ("blank-answer", "   ", 0),
            ("null", "null", 0),
            ("empty-array", "[]", 0),
            ("one-aggregate", r#"[{"batch_len":3}]"#, 1),
            ("three-events", r#"[{"a":1},{"a":2},{"a":3}]"#, 3),
            ("bare-object", r#"{"a":1}"#, 1),
            ("bare-string", r#""text""#, 1),
        ];
        for (case, answer, expected) in rows {
            let events = enrichment_events(answer.as_bytes(), "arn:aws:lambda:us-west-2::function:e")
                .unwrap_or_else(|error| panic!("case {case:?}: {error}"));
            assert_eq!(
                events.len(),
                expected,
                "case {case:?}: expected {expected} event(s), got {events:?}"
            );
        }
    }

    /// RULE: an answer that is not JSON at all is a failed enrichment, not zero events —
    /// the batch must not be reported as delivered.
    #[test]
    fn an_enrichment_answer_that_is_not_json_is_a_failure() {
        let outcome = enrichment_events(b"not json", "arn:aws:lambda:us-west-2::function:e");
        assert!(
            outcome.is_err(),
            "case not-json: expected a failure, got {outcome:?}"
        );
    }

    /// RULE: a target message carries ONE event: the text of a string event, the JSON of
    /// any other — never a wrapper this emulator invented.
    #[test]
    fn a_target_body_is_the_one_event_it_delivers() {
        let rows: Vec<(&str, Value, &str)> = vec![
            ("string", json!("hello"), "hello"),
            ("object", json!({"a":1}), r#"{"a":1}"#),
            ("number", json!(7), "7"),
        ];
        for (case, event, expected) in rows {
            let body = target_body(&event);
            assert_eq!(
                body, expected,
                "case {case:?}: expected {expected:?}, got {body:?}"
            );
        }
    }
}
