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
//!   messages of one event each, never one message holding an array. A parameter's value
//!   is used as configured unless it is a JSON path into the event (`$.…`), which is
//!   resolved against each event delivered, so the value can be the record's own.
//!
//! Either of those last two stages may be handed something OTHER than the event: an
//! `InputTemplate` is what that stage receives instead, rendered per event from the
//! values its `<$.…>` placeholders select in the event, so a target configured with one
//! receives the payload the template spells out rather than the record envelope
//! ([`render_template`]).
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

/// What a target parameter's value starts with when it is a JSON path INTO THE EVENT
/// rather than a literal value: EventBridge Pipes resolves such a value against each
/// event it delivers, so one pipe can take a per-record value from the record itself.
///
/// `PipeTargetParameters`' own documentation names this form — "how to use dynamic path
/// parameters" — as part of what a pipe's target parameters are.
const DYNAMIC_PATH_PREFIX: &str = "$.";

/// The `MessageGroupId` target parameter, named as the model names it — what a
/// resolution failure of that parameter is reported by.
const MESSAGE_GROUP_ID: &str = "MessageGroupId";

/// The `MessageDeduplicationId` target parameter, likewise.
const MESSAGE_DEDUPLICATION_ID: &str = "MessageDeduplicationId";

/// What an `InputTemplate` OPENS a placeholder with, before the JSON path of the value
/// the placeholder stands for.
const PLACEHOLDER_OPEN: char = '<';

/// What it CLOSES one with.
const PLACEHOLDER_CLOSE: char = '>';

/// The path that selects the WHOLE event, which a template uses to pass the event on
/// unchanged.
const WHOLE_EVENT_PATH: &str = "$";

/// The character a JSON string opens and closes with, as the text a rendering trims it
/// from.
const JSON_STRING_QUOTE: &str = "\"";

/// That same character, for the walk that counts the quotes before a placeholder.
const JSON_STRING_QUOTE_CHARACTER: char = '"';

/// The character that escapes the next one inside a JSON string.
const JSON_STRING_ESCAPE: char = '\\';

/// The most bytes of template one render walks.
///
/// The model states this bound on the member itself (`InputTemplate` is
/// `@length(max: 8192)`) and the request boundary enforces it, so this is that same
/// bound checked where the walk RELIES on it: the template is read from stored state, and
/// a render is refused rather than walked when the text it was handed is longer than a
/// template may be.
const MAX_TEMPLATE_BYTES: usize = 8192;

/// The most one event's render may come to.
///
/// This is the EMULATOR's bound, not an AWS number: a template is bounded (8192 bytes)
/// but it may name the same value of the event many times, so what it renders is not
/// bounded by its own length. 256 KiB is what an Amazon SQS message carries at most
/// ("The minimum size is 1 byte. The maximum is 262,144 bytes (256 KiB)",
/// `SendMessage`), so a render past it is a payload no delivery could take: the render
/// fails and the record stays on the source rather than the batch growing the heap.
const MAX_RENDERED_PAYLOAD: usize = 256 * 1024;

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
    // the polled batch itself when it has none. An enrichment is handed the records
    // rendered through ITS input template, when the pipe carries one, because that
    // template is what it receives instead of the record.
    let events = match &run.enrichment {
        None => batch,
        Some(enrichment) => {
            let handed = match transformed(run.enrichment_template.as_deref(), batch) {
                Ok(handed) => handed,
                Err(detail) => return Batch::Failed(detail),
            };
            match invoke_enrichment(callback, clients, enrichment, &handed).await {
                Ok(events) => events,
                Err(detail) => return Batch::Failed(detail),
            }
        }
    };

    // One target call per event, never one call holding the array. What the target
    // RECEIVES is the event rendered through the TARGET's input template, when the pipe
    // carries one; the target parameters are read from the event itself either way.
    for event in &events {
        let payload = match run.target_template.as_deref() {
            None => ::std::borrow::Cow::Borrowed(event),
            Some(template) => match render_template(template, event) {
                Ok(payload) => ::std::borrow::Cow::Owned(payload),
                Err(detail) => return Batch::Failed(detail),
            },
        };
        if let Err(detail) =
            deliver_event(callback, clients, &run.target, payload.as_ref(), event).await
        {
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
///
/// `payload` is what the target RECEIVES — the event, or the event rendered through the
/// target's input template — while `event` is the event itself, which the target
/// parameters are read from.
async fn deliver_event(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    target: &Target,
    payload: &Value,
    event: &Value,
) -> Result<(), String> {
    match target {
        Target::Queue {
            url,
            region,
            message_group_id,
            message_deduplication_id,
        } => {
            // Each parameter is read for THIS event, so one that is a path into the event
            // contributes the record's own value rather than one constant for every
            // record — which is what lets a FIFO target keep a group or a deduplication
            // token per record.
            let group = resolved_parameter(MESSAGE_GROUP_ID, message_group_id.as_deref(), event)?;
            let deduplication = resolved_parameter(
                MESSAGE_DEDUPLICATION_ID,
                message_deduplication_id.as_deref(),
                event,
            )?;
            let mut request = clients
                .sqs(callback, region)
                .send_message()
                .queue_url(url)
                .message_body(target_body(payload));
            if let Some(group) = group {
                request = request.message_group_id(group);
            }
            if let Some(deduplication) = deduplication {
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

/// The value the target parameter `member` contributes to ONE delivery, or `None` when
/// the pipe configures none.
///
/// A configured value is used AS GIVEN unless it is a JSON path into the event
/// ([`DYNAMIC_PATH_PREFIX`]), which is resolved against the event being delivered through
/// the one bounded reference-path evaluation of this tree
/// (`service_common::json_reference_path`): the one value it names, as the text a message
/// member carries. A path that names no one value of the event, or names something that
/// is not a single value, is a FAILED delivery rather than a message carrying the path
/// itself — the record stays on the source and the pipe tries again, exactly as for a
/// target that refused the event.
fn resolved_parameter(
    member: &str,
    configured: Option<&str>,
    event: &Value,
) -> Result<Option<String>, String> {
    let Some(configured) = configured else {
        return Ok(None);
    };
    if !configured.starts_with(DYNAMIC_PATH_PREFIX) {
        return Ok(Some(configured.to_string()));
    }
    let selected = ::service_common::json_reference_path::select(configured, event).ok_or_else(
        || format!("the target parameter {member} {configured} names no one value of the event"),
    )?;
    match selected {
        Value::String(text) => Ok(Some(text.clone())),
        Value::Number(number) => Ok(Some(number.to_string())),
        Value::Bool(flag) => Ok(Some(flag.to_string())),
        other => Err(format!(
            "the target parameter {member} {configured} names a {}, which is not a value a \
             message carries",
            kind_of(other)
        )),
    }
}

/// What KIND of value a path named, for a failure that must not copy the event into its
/// own text.
fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
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

/// Every event of `batch` rendered through `template`, or the batch itself when the
/// stage carries no template.
fn transformed(template: Option<&str>, batch: Vec<Value>) -> Result<Vec<Value>, String> {
    let Some(template) = template else {
        return Ok(batch);
    };
    batch
        .iter()
        .map(|event| render_template(template, event))
        .collect()
}

/// One event rendered through an `InputTemplate`: what the stage the template belongs to
/// is handed INSTEAD of the event.
///
/// "Valid JSON text passed to the target. In this case, nothing from the event itself is
/// passed to the target" (`PipeTargetParameters$InputTemplate`, and the same sentence for
/// the enrichment's) — so the template, not the record, is the payload, and the only parts
/// of the event that reach the stage are the ones the template reads.
///
/// A template is text carrying placeholders — a JSON path between
/// [`PLACEHOLDER_OPEN`] and [`PLACEHOLDER_CLOSE`] — each standing for the value that path
/// selects in the event. A template with no placeholder is a constant, which is what a
/// stage handed the same payload for every event is configured with.
///
/// The rendering is the one a template is WRITTEN for: a placeholder contributes the JSON
/// of the value it selected, so a template that is JSON text renders to JSON (a
/// placeholder inside a quoted string contributes the string's own text, the quotes of
/// the template doing the quoting), and a template that is not JSON text renders to the
/// text it spells out, each placeholder contributing its value's own text. A path that
/// selects no one value of the event is a FAILED render — the record stays on the source
/// — rather than a payload carrying the path itself.
///
/// The text is walked ONCE, left to right, and its length is bounded
/// ([`MAX_TEMPLATE_BYTES`]), so neither the walk nor what it builds is unbounded; what
/// the placeholders ADD is bounded by [`MAX_RENDERED_PAYLOAD`], because a template may
/// name the same value many times. The JSON it renders to is read by `serde_json` and
/// each path by `service_common::json_reference_path`, not by anything spelled out here.
fn render_template(template: &str, event: &Value) -> Result<Value, String> {
    if template.len() > MAX_TEMPLATE_BYTES {
        return Err(format!(
            "the input template is {} bytes, past the {MAX_TEMPLATE_BYTES} a template carries",
            template.len()
        ));
    }
    let placeholders = placeholders_of(template);
    if placeholders.is_empty() {
        // A constant template: the JSON it is, or the text it is.
        return Ok(as_json_or_text(template));
    }
    let mut as_json = String::with_capacity(template.len());
    let mut as_text = String::with_capacity(template.len());
    let mut taken = 0usize;
    for (span, path) in &placeholders {
        let selected = selected_value(path, event)?;
        // Inside a quoted string the template supplies the quotes, so the placeholder
        // contributes the value's text escaped as a JSON string body; anywhere else it
        // contributes the whole JSON of the value.
        let text = value_text(selected);
        let json = if inside_a_json_string(template, span.start) {
            json_string_body(&text)
        } else {
            selected.to_string()
        };
        let before = &template[taken..span.start];
        // Checked BEFORE either buffer grows, so what the render holds never passes the
        // bound rather than passing it and being refused afterwards.
        within_the_payload_bound(as_json.len() + before.len() + json.len())?;
        within_the_payload_bound(as_text.len() + before.len() + text.len())?;
        as_json.push_str(before);
        as_json.push_str(&json);
        as_text.push_str(before);
        as_text.push_str(&text);
        taken = span.end;
    }
    let rest = &template[taken..];
    within_the_payload_bound(as_json.len().max(as_text.len()) + rest.len())?;
    as_json.push_str(rest);
    as_text.push_str(rest);
    Ok(match ::serde_json::from_str::<Value>(&as_json) {
        Ok(value) => value,
        // Not JSON text: the template spells out text, and the text it spells out is what
        // the stage is handed.
        Err(_) => Value::String(as_text),
    })
}

/// Whether a render of `bytes` is one this emulator delivers, or why it is not.
fn within_the_payload_bound(bytes: usize) -> Result<(), String> {
    if bytes > MAX_RENDERED_PAYLOAD {
        return Err(format!(
            "the input template renders more than {MAX_RENDERED_PAYLOAD} bytes for one event, \
             which is more than a payload this emulator delivers"
        ));
    }
    Ok(())
}

/// A template with no placeholder, as the payload it is: the JSON value it is the text
/// of, or the text itself.
fn as_json_or_text(template: &str) -> Value {
    ::serde_json::from_str::<Value>(template).unwrap_or_else(|_| Value::String(template.to_string()))
}

/// The placeholders of `template`, in the order they appear: where each one sits and the
/// JSON path it carries.
///
/// Only a `<…>` span whose content is a JSON path ([`DYNAMIC_PATH_PREFIX`], or the event
/// itself) is a placeholder, so a template carrying `<` for any other reason is left
/// alone — and the scan then resumes just PAST that `<`, so a stray one cannot swallow
/// the placeholder behind it.
fn placeholders_of(template: &str) -> Vec<(std::ops::Range<usize>, String)> {
    let mut found: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut rest = template;
    let mut base = 0usize;
    while let Some(open) = rest.find(PLACEHOLDER_OPEN) {
        let after_open = open + PLACEHOLDER_OPEN.len_utf8();
        let Some(close) = rest[after_open..].find(PLACEHOLDER_CLOSE) else {
            break;
        };
        let end = after_open + close;
        let content = rest[after_open..end].trim();
        let consumed = if content == WHOLE_EVENT_PATH || content.starts_with(DYNAMIC_PATH_PREFIX) {
            found.push((
                base + open..base + end + PLACEHOLDER_CLOSE.len_utf8(),
                content.to_string(),
            ));
            end + PLACEHOLDER_CLOSE.len_utf8()
        } else {
            // Not a placeholder: only this `<` is consumed, because the span it opened
            // may hold the real one.
            after_open
        };
        base += consumed;
        rest = &rest[consumed..];
    }
    found
}

/// Whether the placeholder that starts at `upto` sits INSIDE a quoted string of the
/// template, so the template's own quotes are what quote the value.
///
/// Decided by walking the text before it and counting the quotes that open and close a
/// JSON string (a backslash escapes the next character inside one), so a placeholder with
/// text around it inside the same string is read as quoted too.
fn inside_a_json_string(template: &str, upto: usize) -> bool {
    let mut quoted = false;
    let mut escaped = false;
    for character in template[..upto].chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            JSON_STRING_ESCAPE if quoted => escaped = true,
            JSON_STRING_QUOTE_CHARACTER => quoted = !quoted,
            _ => {}
        }
    }
    quoted
}

/// The ONE value `path` names in `event`, or why the render fails.
///
/// Read through `service_common::json_reference_path`, the one bounded evaluation of a
/// caller-supplied path in this tree, so a template cannot ask for a walk of the whole
/// event.
fn selected_value<'a>(path: &str, event: &'a Value) -> Result<&'a Value, String> {
    ::service_common::json_reference_path::select(path, event)
        .ok_or_else(|| format!("the input template's {path} names no one value of the event"))
}

/// A selected value as its own TEXT: the contents of a string, the JSON of anything else
/// (a number, a boolean, an object or an array has no text but its JSON).
fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `text` as the BODY of a JSON string — its JSON encoding without the surrounding
/// quotes — so a template's own quotes quote it, whatever characters it carries.
fn json_string_body(text: &str) -> String {
    let encoded = Value::String(text.to_string()).to_string();
    encoded
        .strip_prefix(JSON_STRING_QUOTE)
        .and_then(|rest| rest.strip_suffix(JSON_STRING_QUOTE))
        .unwrap_or(&encoded)
        .to_string()
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
    /// `EnrichmentParameters.InputTemplate`: what the ENRICHMENT is handed instead of the
    /// record, rendered per record. Absent when the pipe configures none.
    enrichment_template: Option<String>,
    /// `TargetParameters.InputTemplate`: what the TARGET is handed instead of the event,
    /// rendered per event. Absent when the pipe configures none.
    target_template: Option<String>,
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
        /// `MessageGroupId` and `MessageDeduplicationId` AS THE PIPE CONFIGURES THEM —
        /// each a literal value or a JSON path into the event, read for every delivery by
        /// [`resolved_parameter`] rather than fixed when the pipe is resolved.
        message_group_id: Option<String>,
        message_deduplication_id: Option<String>,
    },
}

/// What this record asks the stages to do, or why this data plane does not serve it.
///
/// A configuration it cannot serve FAITHFULLY leaves the source untouched rather than
/// delivering something the pipe did not ask for: a filter that would drop records, or a
/// source or target of a kind no stage here addresses.
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

    let enrichment = match pipe
        .enrichment
        .as_deref()
        .filter(|arn| !arn.is_empty())
    {
        None => None,
        Some(arn) => Some(resolve_enrichment(arn)?),
    };
    // A template is what the stage it belongs to is handed instead of the event; an empty
    // one configures nothing.
    let enrichment_template = pipe
        .enrichment_parameters
        .as_ref()
        .and_then(|parameters| parameters.input_template.clone())
        .filter(|template| !template.is_empty());

    let target_parameters = pipe.target_parameters.as_ref();
    let target_template = target_parameters
        .and_then(|parameters| parameters.input_template.clone())
        .filter(|template| !template.is_empty());
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
        enrichment_template,
        target_template,
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
    use super::{
        enrichment_events, render_template, resolved_parameter, target_body,
        MAX_TEMPLATE_BYTES, MESSAGE_GROUP_ID,
    };
    use ::serde_json::{json, Value};

    /// The record envelope the template rows below are rendered against: an Amazon SQS
    /// record as the source stage builds one.
    fn record() -> Value {
        json!({
            "messageId": "4fd2-1",
            "body": "order-7",
            "attributes": { "MessageGroupId": "orders", "SequenceNumber": 7 },
            "eventSource": "aws:sqs",
        })
    }

    /// RULE: an `InputTemplate` is what the stage it belongs to is handed INSTEAD of the
    /// event: each `<$.…>` placeholder contributes the value that path selects in the
    /// event, a template that renders JSON text is handed over as that JSON, and one that
    /// does not is handed over as the text it spells out.
    ///
    /// The rows differ along the form of the template, which is what the rule generalises
    /// over: one bare placeholder, a quoted one, placeholders inside a JSON object, a
    /// template with no placeholder at all, and text around a placeholder.
    #[test]
    fn an_input_template_is_what_the_stage_is_handed() {
        let event = record();
        // (case, the template, what the stage is handed)
        let rows: Vec<(&str, &str, Value)> = vec![
            ("the-body-alone", "<$.body>", json!("order-7")),
            ("the-body-quoted", "\"<$.body>\"", json!("order-7")),
            ("the-whole-event", "<$>", record()),
            (
                "an-object-of-two-values",
                r#"{"order": "<$.body>", "from": "<$.attributes.MessageGroupId>"}"#,
                json!({"order": "order-7", "from": "orders"}),
            ),
            (
                "an-unquoted-number",
                r#"{"seq": <$.attributes.SequenceNumber>}"#,
                json!({"seq": 7}),
            ),
            (
                "a-nested-value",
                r#"{"group": <$.attributes.MessageGroupId>}"#,
                json!({"group": "orders"}),
            ),
            ("a-constant-object", r#"{"fixed": true}"#, json!({"fixed": true})),
            ("a-constant-text", "nothing to read", json!("nothing to read")),
            (
                "text-around-a-placeholder",
                "order <$.body> received",
                json!("order order-7 received"),
            ),
            // Inside a JSON string with text around it: the template's own quotes quote
            // the value, so the render is that string.
            (
                "a-quoted-placeholder-with-text",
                "\"order <$.body> received\"",
                json!("order order-7 received"),
            ),
            (
                "a-quoted-placeholder-with-text-in-an-object",
                r#"{"text": "order <$.body> from <$.attributes.MessageGroupId>"}"#,
                json!({"text": "order order-7 from orders"}),
            ),
        ];
        for (case, template, expected) in rows {
            let rendered = render_template(template, &event)
                .unwrap_or_else(|error| panic!("case {case:?}: {error}"));
            assert_eq!(
                rendered, expected,
                "case {case:?}: expected {expected}, got {rendered}"
            );
        }
    }

    /// RULE: a template whose placeholder selects no one value of the event renders
    /// nothing — the record stays on the source — rather than a payload carrying the path
    /// itself.
    #[test]
    fn a_template_placeholder_that_selects_nothing_fails_the_render() {
        let event = record();
        // (case, the template)
        let rows: Vec<(&str, &str)> = vec![
            ("absent", "<$.detail>"),
            ("absent-inside-an-object", r#"{"x": <$.attributes.Absent>}"#),
        ];
        for (case, template) in rows {
            let outcome = render_template(template, &event);
            assert!(
                outcome.is_err(),
                "case {case:?}: expected the render to fail, got {outcome:?}"
            );
        }
    }

    /// RULE: what a template renders for ONE event is bounded, however many times it
    /// names a value of that event, and a template longer than one may carry is refused
    /// rather than walked.
    #[test]
    fn a_template_that_renders_too_much_fails_the_render() {
        // A big event, named by as many placeholders as a bounded template holds: each
        // one is three bytes of template and a whole event of payload.
        let event = json!({ "body": "x".repeat(8 * 1024) });
        let template = "<$>".repeat(2 * 1024);
        let outcome = render_template(&template, &event);
        assert!(
            outcome.is_err(),
            "case amplified: expected the render to be refused, got {} byte(s)",
            outcome.map(|value| value.to_string().len()).unwrap_or(0)
        );
        // And the template's own length is a bound of its own.
        let too_long = "a".repeat(MAX_TEMPLATE_BYTES + 1);
        let outcome = render_template(&too_long, &event);
        assert!(
            outcome.is_err(),
            "case past-the-template-bound: expected the render to be refused, got {outcome:?}"
        );
        let at_the_bound = "a".repeat(MAX_TEMPLATE_BYTES);
        let outcome = render_template(&at_the_bound, &event);
        assert!(
            outcome.is_ok(),
            "case at-the-template-bound: expected a template of {MAX_TEMPLATE_BYTES} bytes to \
             render, got {outcome:?}"
        );
    }

    /// RULE: a `<…>` span that is not a JSON path is not a placeholder, so a template may
    /// carry the characters for any other reason and is handed over as written — and a
    /// stray `<` does not swallow the placeholder behind it.
    #[test]
    fn a_span_that_is_not_a_path_is_left_as_written() {
        let event = record();
        // (case, the template, what the stage is handed)
        let rows: Vec<(&str, &str, Value)> = vec![
            ("not-a-path", "a < b and c > d", json!("a < b and c > d")),
            (
                "a-stray-open-before-a-placeholder",
                "a < b <$.body>",
                json!("a < b order-7"),
            ),
            (
                "a-stray-open-inside-an-object",
                r#"{"text": "a < b <$.body>"}"#,
                json!({"text": "a < b order-7"}),
            ),
            // A placeholder nothing closes is not one either: the template is text.
            (
                "an-unterminated-placeholder",
                "<$.body",
                json!("<$.body"),
            ),
        ];
        for (case, template, expected) in rows {
            let rendered = render_template(template, &event)
                .unwrap_or_else(|error| panic!("case {case:?}: {error}"));
            assert_eq!(
                rendered, expected,
                "case {case:?}: expected {expected}, got {rendered}"
            );
        }
    }

    /// RULE: a target parameter's value is used AS CONFIGURED unless it is a JSON path
    /// into the event, in which case the value the path selects in the event is what the
    /// delivery carries.
    ///
    /// The rows differ along the form of the configured value and the type of the node a
    /// path selects, which is what the rule generalises over.
    #[test]
    fn a_target_parameter_is_the_literal_or_what_its_path_selects() {
        let event = json!({
            "messageId": "4fd2-1",
            "body": "{\"n\":1}",
            "attributes": { "MessageGroupId": "orders", "SequenceNumber": 7 },
            "deep": { "flag": true },
        });
        // (case, configured value, the value one delivery carries)
        let rows: Vec<(&str, Option<&str>, Option<&str>)> = vec![
            ("configures-none", None, None),
            ("a-literal", Some("orders"), Some("orders")),
            // A literal is never read as a path, even when it looks like one in part.
            ("a-literal-with-a-dollar", Some("group$1"), Some("group$1")),
            ("a-path-to-a-string", Some("$.messageId"), Some("4fd2-1")),
            (
                "a-path-into-an-object",
                Some("$.attributes.MessageGroupId"),
                Some("orders"),
            ),
            (
                "a-path-to-a-number",
                Some("$.attributes.SequenceNumber"),
                Some("7"),
            ),
            ("a-path-to-a-boolean", Some("$.deep.flag"), Some("true")),
        ];
        for (case, configured, expected) in rows {
            let resolved = resolved_parameter(MESSAGE_GROUP_ID, configured, &event)
                .unwrap_or_else(|error| panic!("case {case:?}: {error}"));
            assert_eq!(
                resolved.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {resolved:?}"
            );
        }
    }

    /// RULE: a path that does not select ONE value of the event is a failed delivery —
    /// the record stays on the source — and never a message carrying the path itself.
    #[test]
    fn a_path_that_selects_no_one_value_fails_the_delivery() {
        let event = json!({ "a": [1, 2], "b": { "c": "x" } });
        // (case, the configured path)
        let rows: Vec<(&str, &str)> = vec![
            ("absent", "$.missing"),
            ("several", "$.a[*]"),
            ("an-array", "$.a"),
            ("an-object", "$.b"),
        ];
        for (case, configured) in rows {
            let outcome = resolved_parameter(MESSAGE_GROUP_ID, Some(configured), &event);
            assert!(
                outcome.is_err(),
                "case {case:?}: expected the delivery to fail, got {outcome:?}"
            );
        }
    }

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
