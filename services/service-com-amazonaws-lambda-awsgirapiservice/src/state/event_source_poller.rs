// SPDX-License-Identifier: CC0-1.0

//! The DATA PLANE of an event source mapping on an Amazon Kinesis DATA STREAM: what such
//! a mapping does with no client action at all.
//!
//! A mapping's control plane (the record, its UUID and ARN, its lifecycle states) lives in
//! [`crate::state`]. This module is the other half for the stream sources whose records sit
//! in SHARDS, as [`crate::state::sqs_poller`] is for a mapping on a queue: once a mapping on
//! a Kinesis data stream is `Enabled`, Lambda itself reads that stream's shards and invokes
//! the mapped function with the batch it read — no caller asks it to. The two sources need
//! their own readers because what each hands over, and how a reader keeps its place in it,
//! differ: a queue hides a message it hands over for its own visibility timeout, while a
//! shard hands out the records after a POSITION the reader carries. The rules this one
//! follows:
//!
//! * WHILE PAUSED, NOTHING IS READ. "When true, the event source mapping is active. When
//!   false, Lambda pauses polling and invocation" (`Enabled`), so a `Disabled` mapping
//!   reads no record and invokes nothing, and the records put to the stream meanwhile stay
//!   in the shard for its retention period.
//! * THE POSITION IS THE MAPPING'S. `StartingPosition` is "The position in a stream from
//!   which to start reading", a property of the mapping fixed when the mapping is created,
//!   so pausing does not move it ([`crate::state::ShardCursor`]): a mapping enabled again
//!   reads on from where it had got to, which for one that read nothing yet is where its
//!   creation fixed it. That is what makes a backlog built up while it was paused
//!   OBSERVABLE rather than skipped.
//! * EACH BATCH READ IS METERED. The age of the newest record of the batch, at the instant
//!   the batch was read off the shard, is published as this function's
//!   `IteratorAge` ([`crate::state::metrics`]) — so a batch that sat in the
//!   shard while the mapping was paused meters that whole wait, and a batch read as its
//!   records arrive meters almost nothing.
//! * A BATCH IS CHECKPOINTED PAST ONLY ONCE DELIVERED. An invocation that did not succeed
//!   leaves the cursor where it was, so the same batch is read again — "The default value
//!   is infinite (-1). When set to infinite (-1), failed records are retried until the
//!   record expires" (`MaximumRetryAttempts`) — and what the attempt came to is reported as
//!   the mapping's own `LastProcessingResult`. A mapping whose `FunctionResponseTypes` asks
//!   for its function's report of which records of the batch failed is checkpointed past
//!   only the records at the head of the batch that report leaves out, so the first record
//!   it names is where the next read starts (see [`super::batch_item_failures`]).
//!
//! * NOTHING IS READ FOR A FUNCTION THAT CANNOT BE INVOKED. "Lambda reads items from the
//!   event source and invokes the function" (`CreateEventSourceMapping`), so a mapping
//!   whose function this store does not hold `Active` takes nothing off a shard — the same
//!   rule [`crate::state::sqs_poller`] holds a queue mapping to.
//! * THE SHARDS ARE READ AS THE FUNCTION'S EXECUTION ROLE. Lambda reads the source acting as
//!   that role, so while the policies in force on it withhold one of the reads consuming the
//!   stream takes, a pass reads nothing and no cursor moves
//!   ([`super::event_source_authorization`]) — the records stay in the shard until the grant
//!   is in place, which is the same question the call that WIRED the pair together was
//!   refused for, and again the rule [`crate::state::sqs_poller`] holds a queue mapping to.
//!
//! Each pass reads what the shard holds and hands it over at once, so a mapping that
//! configures a `MaximumBatchingWindowInSeconds` above the Kinesis default of 0 is not
//! waited out here: that yields SMALLER batches than such a mapping would see on AWS,
//! never a record that is not delivered.
//!
//! Every call this module makes goes to the emulator it is running inside, over the
//! in-process callback ([`smithy_server_runtime::EmbeddedEmulator`]): the stream is read
//! through the Kinesis surface of this same emulator (`ListShards`, `GetShardIterator`,
//! `GetRecords`) and the function is invoked through its own (`Invoke`), so nothing is
//! read or delivered that this emulator does not serve itself, and a record is consumed
//! only through the data plane a client would consume it through.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use ::base64::Engine as _;
use ::serde_json::{json, Value};

use super::{metrics::FunctionMetrics, ComAmazonawsLambdaAwsgirapiserviceState, ShardCursor};

/// How long the poller waits before looking at an idle shard again. Short enough that a
/// record put on the stream is delivered promptly, long enough not to spin.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long it waits before looking at a mapping that is NOT reading again — one that
/// rests `Disabled`, or is still settling into the state its `Enabled` setting asks for.
/// Longer than [`POLL_INTERVAL`], because such a mapping has no shard to look at: the one
/// thing a pass does is read the mapping's own state, and nothing but a call can change
/// it.
const PAUSED_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// How long the poller waits after a failed attempt before trying the batch again. It is
/// multiplied by the number of consecutive failures, up to [`MAX_RETRY_BACKOFF`].
const RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// The longest the poller waits between two attempts at a batch it keeps failing to
/// deliver.
///
/// A mapping whose stream or function cannot be reached keeps trying — that is what lets
/// it recover when the resource comes back — but it does so at this interval rather than
/// at the rate a healthy mapping polls.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(5);

/// How often a mapping that keeps failing says so: the first failure, and every this many
/// after it, so one broken mapping cannot fill the log.
const FAILURE_LOG_INTERVAL: u32 = 20;

/// The most records this emulator reads in ONE batch, whatever `BatchSize` the mapping
/// configures.
///
/// This is the EMULATOR's bound, not an AWS number (`BatchSize` itself reaches 10000): a
/// batch is held in memory and the whole of it becomes one invocation payload, so the
/// count a caller chose is clamped before anything is read. A mapping configured above it
/// reads this many records per batch instead, which is a smaller batch and never a dropped
/// record.
const MAX_RECORDS_PER_BATCH: i32 = 1_000;

/// The most `ListShards` pages one pass walks, so a stream cannot keep the poller in its
/// shard listing.
const MAX_SHARD_PAGES: usize = 10;

/// The `eventSource` a Kinesis record envelope reports, and the `eventName` each record
/// carries, as the Kinesis event Lambda hands a function spells them.
const KINESIS_EVENT_SOURCE: &str = "aws:kinesis";
/// See [`KINESIS_EVENT_SOURCE`].
const KINESIS_EVENT_NAME: &str = "aws:kinesis:record";
/// The schema version of the `kinesis` member of one record envelope.
const KINESIS_SCHEMA_VERSION: &str = "1.0";
/// The version of the record envelope itself.
const KINESIS_EVENT_VERSION: &str = "1.0";

/// Start the in-process poller for the event source mapping `uuid` names, if one is not
/// already running for it.
///
/// Called by an operation that has created or updated the mapping, AFTER it has released
/// the store's lock. It changes nothing a caller is told: the poller observes the
/// mapping's own `State` and reads nothing until the clock has moved it to `Enabled`. With
/// no async runtime to spawn onto there is no poller and nothing is polled.
///
/// At most ONE task runs per mapping, and the store holds at most
/// [`super::MAX_EVENT_SOURCE_MAPPINGS`] mappings, so the tasks this can ever spawn are
/// bounded by that count. A task ends as soon as its mapping is gone or being deleted, so
/// none of them outlives the mapping it polls, and its claim is released however it ends
/// (see [`PollerClaim`]).
pub fn drive_mapping<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &::std::sync::Arc<Eng>,
    uuid: &str,
) {
    let Some(state) = super::service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    {
        let mut store = state.lock();
        // The source this module reads is an Amazon Kinesis data stream; a mapping on any
        // other source has no reader here, so no task is started for it at all.
        let reads_kinesis = store
            .event_source_mappings
            .get(uuid)
            .and_then(|mapping| mapping.event_source_arn.clone())
            .as_deref()
            .and_then(super::event_source_vendor)
            == Some(super::EVENT_SOURCE_VENDOR_KINESIS);
        if !reads_kinesis || !store.claim_event_source_poller(uuid) {
            return;
        }
    }
    let callback = ctx.callback().clone();
    // The CloudWatch store this mapping's work is published to, resolved once per poller
    // because the handle does not change.
    let metrics = FunctionMetrics::new(super::metrics::cloudwatch_state(&**engine));
    // The IAM store the function's execution role is judged against, resolved once for the
    // same reason. An engine carrying none shows nothing about the role.
    let iam = super::event_source_authorization::iam_state(&**engine);
    let claim = PollerClaim {
        state: ::engine::ServiceState::clone(&state),
        uuid: uuid.to_string(),
        held: true,
    };
    runtime.spawn(async move {
        poll_mapping(&callback, claim, metrics, iam).await;
    });
}

/// The claim of the one poller of a mapping, held for as long as that poller runs.
///
/// It is released by [`Self::release`] inside the critical section that decided the poller
/// is done, so a call that arrives afterwards starts a new poller. The `Drop` is the
/// backstop for every OTHER way the task can end — the runtime shutting down, or a panic
/// inside the poller — because a claim left behind would stop this mapping from ever being
/// polled again.
struct PollerClaim {
    state: ::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    uuid: String,
    /// Whether this claim is still the live one. Set to `false` by [`Self::release`], so
    /// the `Drop` cannot release a claim a LATER poller has since taken.
    held: bool,
}

impl PollerClaim {
    /// Release the claim through a store lock the caller already holds.
    fn release(&mut self, store: &mut ComAmazonawsLambdaAwsgirapiserviceState) {
        if self.held {
            store.release_event_source_poller(&self.uuid);
            self.held = false;
        }
    }
}

impl Drop for PollerClaim {
    fn drop(&mut self) {
        if self.held {
            let mut store = self.state.lock();
            store.release_event_source_poller(&self.uuid);
            self.held = false;
        }
    }
}

/// One mapping's configuration as ONE pass reads it: everything the pass needs, taken from
/// the record under the store's lock and never held across a call.
#[derive(Debug)]
struct KinesisSourcedMapping {
    /// The stream the shards are read from.
    stream_arn: String,
    /// The Region that stream lives in, which is the Region it is reached in: a resource
    /// belongs to one Region's namespace.
    region: String,
    /// The function the batch is handed to, as the mapping resolved it.
    function_arn: String,
    /// That function's bare name, which is the dimension its metrics are published per.
    function_name: String,
    /// The execution role the invocation is attributed to, reported to the function as
    /// `invokeIdentityArn`; `None` when this store holds no configuration to read it from.
    invoke_identity_arn: Option<String>,
    /// How many records one batch holds at most, from `BatchSize`, clamped to what this
    /// emulator reads in one go ([`MAX_RECORDS_PER_BATCH`]).
    batch_size: i32,
    /// Whether the mapping's `FunctionResponseTypes` asks for the function's report of which
    /// records of the batch failed to be read (see [`super::batch_item_failures`]).
    reports_batch_item_failures: bool,
    /// Where the mapping reads next in each shard it has already read from.
    cursors: BTreeMap<String, ShardCursor>,
    /// Where it reads a shard it has not read from yet.
    initial_cursor: ShardCursor,
}

/// What the poller may do on one pass, decided under the store's lock.
enum Pass {
    /// The mapping is `Enabled`: read what its shards hold under this configuration. Boxed
    /// because it is far larger than the other answers, which most passes are.
    Run(Box<KinesisSourcedMapping>),
    /// The mapping is not reading: it is still settling into the state its `Enabled`
    /// setting asks for, it rests `Disabled`, which "pauses polling and invocation", or the
    /// function it invokes is not `Active`, so there is no invocation to read a batch for.
    /// Nothing is read and the poller stays, so the mapping resumes where it left off.
    Paused,
    /// The mapping is `Enabled` but names an event source this reader cannot read, and
    /// why. Nothing is read and the poller ends.
    Unserved(String),
    /// The mapping is gone or being deleted. The claim is already released.
    Stop,
}

/// What ONE pass over the mapping's shards came to.
enum Outcome {
    /// Every batch read was delivered, so there may be more waiting.
    Delivered,
    /// Every shard held nothing to read.
    Idle,
    /// A call failed, or an invocation did not succeed, for at least one shard. That
    /// shard was not checkpointed past, so its records are read again.
    Failed(String),
}

/// Poll one mapping until it is gone.
///
/// The store's lock is taken for the decision of each pass and released before any call
/// leaves this process, so a request never waits on a delivery.
async fn poll_mapping(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    mut claim: PollerClaim,
    metrics: FunctionMetrics,
    iam: Option<::engine::ServiceState<super::event_source_authorization::IamState>>,
) {
    let uuid = claim.uuid.clone();
    // A handle of its own, so the store can be locked while the claim is borrowed.
    let state = ::engine::ServiceState::clone(&claim.state);
    let mut clients = Clients::default();
    let mut failures = 0u32;
    loop {
        let pass = {
            let mut store = state.lock();
            // The state moves the clock has ALREADY made due, so a mapping whose
            // `Creating` span has lapsed is seen as the `Enabled` mapping it became.
            store.materialize_all(SystemTime::now());
            next_pass(&mut store, &mut claim)
        };
        match pass {
            Pass::Stop => return,
            Pass::Paused => ::tokio::time::sleep(PAUSED_POLL_INTERVAL).await,
            Pass::Unserved(detail) => {
                ::tracing::warn!(
                    mapping = %uuid,
                    detail = %detail,
                    "this emulator reads no event source of that kind, so the mapping's \
                     source is left untouched"
                );
                let mut store = state.lock();
                claim.release(&mut store);
                return;
            }
            Pass::Run(run) => {
                // Lambda reads the shards ACTING AS the function's execution role, so this
                // pass's reads are subject to the policies in force on that role AT THIS
                // INSTANT rather than when the pair was wired: while one of them is
                // withheld, nothing is read off a shard — no record is delivered and no
                // cursor moves — and the mapping resumes where it left off once the grant is
                // back (see [`super::event_source_authorization`], including the cases that
                // show nothing about a role and withhold nothing).
                if let Some(detail) = withheld_shard_read(iam.as_ref(), &run) {
                    failures = failures.saturating_add(1);
                    if failures == 1 || failures.is_multiple_of(FAILURE_LOG_INTERVAL) {
                        ::tracing::warn!(
                            mapping = %uuid,
                            attempt = failures,
                            detail = %detail,
                            "an event source mapping may not read its stream as its \
                             function's execution role; the records stay in the shard until \
                             that role is granted the reads"
                        );
                    }
                    ::tokio::time::sleep(retry_backoff(failures)).await;
                    continue;
                }
                match move_batches(callback, &mut clients, &state, &metrics, &uuid, &run).await {
                    Outcome::Delivered => failures = 0,
                    Outcome::Idle => {
                        failures = 0;
                        ::tokio::time::sleep(POLL_INTERVAL).await;
                    }
                    Outcome::Failed(detail) => {
                        failures = failures.saturating_add(1);
                        if failures == 1 || failures.is_multiple_of(FAILURE_LOG_INTERVAL) {
                            ::tracing::warn!(
                                mapping = %uuid,
                                attempt = failures,
                                detail = %detail,
                                "an event source mapping could not move a batch from its \
                                 stream to its function; the records stay in the shard and \
                                 it tries again"
                            );
                        }
                        ::tokio::time::sleep(retry_backoff(failures)).await;
                    }
                }
            }
        }
    }
}

/// Why the reads this pass would make on `run`'s stream are WITHHELD from the execution
/// role of the function the mapping invokes, or `None` when they are not — which includes
/// every case that shows nothing about the role (see
/// [`super::event_source_authorization`]).
///
/// Only the IAM store is locked here, and this service's is not: the decision is taken
/// between two passes rather than inside the critical section either of them is decided in.
fn withheld_shard_read(
    iam: Option<&::engine::ServiceState<super::event_source_authorization::IamState>>,
    run: &KinesisSourcedMapping,
) -> Option<String> {
    let role_arn = run.invoke_identity_arn.as_deref()?;
    super::event_source_authorization::unreadable_event_source_held(
        iam,
        role_arn,
        &run.stream_arn,
    )
}

/// How long to wait after `failures` consecutive failed attempts at one batch: it grows
/// with them, capped at [`MAX_RETRY_BACKOFF`].
fn retry_backoff(failures: u32) -> Duration {
    RETRY_BACKOFF
        .saturating_mul(failures.max(1))
        .min(MAX_RETRY_BACKOFF)
}

/// What the poller does next for the mapping it claims, releasing its claim when the
/// answer is [`Pass::Stop`] — the release and the state it was decided from are read in ONE
/// critical section, so a call that has already re-created a poller's work either keeps
/// this poller or starts a new one, never neither.
fn next_pass(
    store: &mut ComAmazonawsLambdaAwsgirapiserviceState,
    claim: &mut PollerClaim,
) -> Pass {
    let resolved = match store.event_source_mappings.get(&claim.uuid) {
        // "When you delete an event source mapping, it enters a Deleting state": nothing
        // more is read for it, and its removal is owed by the clock.
        None => None,
        Some(mapping) if mapping.is_deleting() => None,
        // Still moving into the state its settings ask for, or resting `Disabled`.
        Some(mapping)
            if mapping.is_settling()
                || mapping.state != super::EVENT_SOURCE_MAPPING_STATE_ENABLED =>
        {
            Some(Pass::Paused)
        }
        // "Lambda reads items from the event source and invokes the function": a mapping
        // whose function this store does not hold `Active` takes nothing off a shard, so
        // no batch is read for an invocation that could not be made.
        Some(mapping)
            if !store
                .functions
                .get(&mapping.function_name)
                .is_some_and(|function| function.state == super::STATE_ACTIVE) =>
        {
            Some(Pass::Paused)
        }
        Some(mapping) => Some(match run_of(store, mapping) {
            Ok(run) => Pass::Run(Box::new(run)),
            // A source this reader cannot read is reported by the pass itself, so the one
            // log line is written outside the lock.
            Err(detail) => Pass::Unserved(detail),
        }),
    };
    match resolved {
        Some(pass) => pass,
        None => {
            claim.release(store);
            Pass::Stop
        }
    }
}

/// The configuration one pass reads under, or why this mapping's source cannot be read
/// here.
fn run_of(
    store: &ComAmazonawsLambdaAwsgirapiserviceState,
    mapping: &super::EventSourceMappingRecord,
) -> Result<KinesisSourcedMapping, String> {
    let Some(stream_arn) = mapping.event_source_arn.clone() else {
        return Err("the mapping names no event source ARN to read".to_string());
    };
    let Some(parsed) = ::service_common::arn::parse(&stream_arn) else {
        return Err("the mapping's event source is not a resource ARN".to_string());
    };
    if parsed.vendor != super::EVENT_SOURCE_VENDOR_KINESIS {
        return Err(format!(
            "an event source of the {} namespace has no reader in this emulator",
            parsed.vendor
        ));
    }
    let region = parsed.region.to_string();
    // The role the invocation is attributed to, read from the VERSION this mapping
    // invokes: "The ARN of the Lambda function" the mapping carries may be qualified, and
    // an alias names the version it invokes.
    let invoke_identity_arn = store.functions.get(&mapping.function_name).and_then(|function| {
        let version = mapping
            .function_qualifier
            .as_deref()
            .and_then(|qualifier| function.resolved_version(qualifier))
            .unwrap_or_else(|| super::VERSION_LATEST.to_string());
        function.config_of(&version).map(|config| config.role.clone())
    });
    Ok(KinesisSourcedMapping {
        stream_arn,
        region,
        function_arn: mapping.function_arn.clone(),
        function_name: mapping.function_name.clone(),
        invoke_identity_arn,
        batch_size: mapping.batch_size.clamp(1, MAX_RECORDS_PER_BATCH),
        reports_batch_item_failures: super::reports_batch_item_failures(
            &mapping.function_response_types,
        ),
        cursors: mapping.shard_cursors.clone(),
        initial_cursor: mapping.initial_shard_cursor(),
    })
}

/// Read one batch from each shard of the mapped stream and hand each to the function.
///
/// Each shard is read on its own — "Process multiple batches from each shard"
/// (`ParallelizationFactor`) is per shard, and a shard's position is its own — so a batch
/// one shard could not deliver does not stop the others from being read on this pass.
///
/// Every batch read is metered before it is handed over, because the age it meters is a
/// property of the READ: it is how long the newest record of the batch had been in the
/// shard at the instant Lambda took it off, whatever the invocation then comes to.
async fn move_batches(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    state: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    metrics: &FunctionMetrics,
    uuid: &str,
    run: &KinesisSourcedMapping,
) -> Outcome {
    let shards = match shard_ids(callback, clients, run).await {
        Ok(shards) => shards,
        Err(detail) => return Outcome::Failed(detail),
    };
    let mut delivered = false;
    let mut failure: Option<String> = None;
    for shard in shards {
        let cursor = run
            .cursors
            .get(&shard)
            .cloned()
            .unwrap_or_else(|| run.initial_cursor.clone());
        let iterator = match shard_iterator(callback, clients, run, &shard, &cursor).await {
            // "If the shard has been closed, the shard iterator can't return more data":
            // such a shard has no position left to read from.
            Ok(None) => continue,
            Ok(Some(iterator)) => iterator,
            Err(detail) => {
                failure = Some(detail);
                continue;
            }
        };
        let records = match records_at(callback, clients, run, &iterator).await {
            Ok(records) => records,
            Err(detail) => {
                failure = Some(detail);
                continue;
            }
        };
        let Some(last) = records.last() else {
            continue;
        };
        // Every record of the batch, by the identifier a report of this source names one by:
        // "the sequence number of the data record in the shard".
        let sequence_numbers: Vec<String> = records
            .iter()
            .map(|record| record.sequence_number().to_string())
            .collect();
        metrics.publish_iterator_age(
            &run.function_name,
            iterator_age_millis(last.approximate_arrival_timestamp(), SystemTime::now()),
        );
        let payload = kinesis_event(&records, &shard, run);
        match hand_over_batch(callback, clients, run, &payload, &sequence_numbers).await {
            Ok(processed) => {
                // The mapping reads on from past the records the attempt processed, and the
                // attempt is reported as what the mapping last came to. A shard has ONE
                // position, so what it reads on from is the run of records at the head of
                // the batch the attempt processed: the first record its function reported it
                // did not process is where the next read starts, whatever the report says
                // about the records behind that one.
                match checkpointed_through(&sequence_numbers, &processed) {
                    Some(sequence_number) => {
                        checkpoint(
                            state,
                            uuid,
                            run,
                            &shard,
                            Some(sequence_number),
                            super::LAST_PROCESSING_RESULT_OK,
                        );
                        delivered = true;
                    }
                    None => {
                        // The report named the batch's own first record, so the cursor stays
                        // where it was and the whole batch is read again: an attempt that
                        // moved the shard's position not at all is one that did not process
                        // the batch, which is what it reports and what the poller backs off
                        // after.
                        checkpoint(
                            state,
                            uuid,
                            run,
                            &shard,
                            None,
                            super::LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED,
                        );
                        failure = Some(format!(
                            "{} reported the first record of the batch it was handed as one \
                             it did not process, so {shard} reads the same batch again",
                            run.function_arn
                        ));
                    }
                }
            }
            Err(detail) => {
                // Not delivered: the cursor stays where it was, so this batch is read
                // again, and the failed attempt is what the mapping last came to. Each
                // shard is read on its own, so the other shards of the stream are still
                // read on this pass.
                checkpoint(
                    state,
                    uuid,
                    run,
                    &shard,
                    None,
                    super::LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED,
                );
                failure = Some(detail);
            }
        }
    }
    match failure {
        Some(detail) => Outcome::Failed(detail),
        None if delivered => Outcome::Delivered,
        None => Outcome::Idle,
    }
}

/// Record what one processing attempt came to on the mapping itself, and — for an attempt
/// that delivered its batch — move the shard's cursor past that batch.
///
/// The record is re-read under the lock and only written when it still names the stream and
/// the function this batch was read for, so a mapping re-targeted while the batch was in
/// flight is not checkpointed under a configuration it no longer carries.
fn checkpoint(
    state: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    uuid: &str,
    run: &KinesisSourcedMapping,
    shard: &str,
    delivered_through: Option<String>,
    result: &str,
) {
    let mut store = state.lock();
    let Some(mapping) = store.event_source_mappings.get_mut(uuid) else {
        return;
    };
    if mapping.event_source_arn.as_deref() != Some(run.stream_arn.as_str())
        || mapping.function_arn != run.function_arn
    {
        return;
    }
    if let Some(sequence_number) = delivered_through {
        mapping
            .shard_cursors
            .insert(shard.to_string(), ShardCursor::AfterSequenceNumber(sequence_number));
    }
    mapping.last_processing_result = Some(result.to_string());
}

/// How long the newest record of a batch had been in the shard when the batch was read,
/// in milliseconds: never negative, and 0 for a record this emulator holds no arrival
/// instant for.
fn iterator_age_millis(
    arrived_at: Option<&::aws_sdk_kinesis::primitives::DateTime>,
    read_at: SystemTime,
) -> f64 {
    let Some(arrived_at) = arrived_at else {
        return 0.0;
    };
    let read_at = ::aws_sdk_kinesis::primitives::DateTime::from(read_at);
    ((read_at.as_secs_f64() - arrived_at.as_secs_f64()) * 1_000.0).max(0.0)
}

/// Every open and closed shard of the mapped stream, in the order the stream reports them.
async fn shard_ids(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &KinesisSourcedMapping,
) -> Result<Vec<String>, String> {
    let mut shards: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..MAX_SHARD_PAGES {
        let mut request = clients.kinesis(callback, &run.region).list_shards();
        // "NextToken" and the stream the first page named are mutually exclusive: a
        // continuation already carries the stream it is a continuation of.
        request = match next_token.take() {
            Some(token) => request.next_token(token),
            None => request.stream_arn(&run.stream_arn),
        };
        let answer = request.send().await.map_err(|error| {
            format!(
                "the shards of {} could not be listed ({})",
                run.stream_arn,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
        shards.extend(
            answer
                .shards()
                .iter()
                .map(|shard| shard.shard_id().to_string()),
        );
        match answer.next_token() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    Ok(shards)
}

/// The iterator that reads `shard` from `cursor`, or `None` for a shard that has no
/// position left to read from.
async fn shard_iterator(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &KinesisSourcedMapping,
    shard: &str,
    cursor: &ShardCursor,
) -> Result<Option<String>, String> {
    use ::aws_sdk_kinesis::types::ShardIteratorType;

    let request = clients
        .kinesis(callback, &run.region)
        .get_shard_iterator()
        .stream_arn(&run.stream_arn)
        .shard_id(shard);
    let request = match cursor {
        ShardCursor::TrimHorizon => request.shard_iterator_type(ShardIteratorType::TrimHorizon),
        ShardCursor::AtTimestamp(instant) => request
            .shard_iterator_type(ShardIteratorType::AtTimestamp)
            .timestamp(::aws_sdk_kinesis::primitives::DateTime::from(*instant)),
        ShardCursor::AfterSequenceNumber(sequence_number) => request
            .shard_iterator_type(ShardIteratorType::AfterSequenceNumber)
            .starting_sequence_number(sequence_number),
    };
    let answer = request.send().await.map_err(|error| {
        format!(
            "no iterator could be taken on {shard} of {} ({})",
            run.stream_arn,
            ::aws_smithy_types::error::display::DisplayErrorContext(&error)
        )
    })?;
    Ok(answer.shard_iterator().map(str::to_string))
}

/// The records one batch holds: what the shard has at `iterator`, up to the mapping's
/// batch size.
async fn records_at(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &KinesisSourcedMapping,
    iterator: &str,
) -> Result<Vec<::aws_sdk_kinesis::types::Record>, String> {
    let answer = clients
        .kinesis(callback, &run.region)
        .get_records()
        .shard_iterator(iterator)
        .limit(run.batch_size)
        .send()
        .await
        .map_err(|error| {
            format!(
                "a batch could not be read from {} ({})",
                run.stream_arn,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    Ok(answer.records)
}

/// Hand one batch to the function and read what became of the records it held: the ones the
/// attempt PROCESSED, or why none of them was.
///
/// Every record of a batch an invocation accepted was processed unless the mapping asks for
/// its function's report of which of them failed to be read, which is what
/// `FunctionResponseTypes` says (see [`super::batch_item_failures`]).
async fn hand_over_batch(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &KinesisSourcedMapping,
    payload: &Value,
    sequence_numbers: &[String],
) -> Result<Vec<String>, String> {
    let answered = invoke_function(callback, clients, run, payload).await?;
    if !run.reports_batch_item_failures {
        return Ok(sequence_numbers.to_vec());
    }
    match super::read_batch_item_failures(answered.as_deref(), sequence_numbers) {
        super::BatchReport::WholeBatchFailed(detail) => Err(format!(
            "{} reported no record of the batch as processed: {detail}",
            run.function_arn
        )),
        report => Ok(super::processed_records(&report, sequence_numbers).unwrap_or_default()),
    }
}

/// The sequence number a shard reads on from after an attempt that processed `processed` of
/// the batch `sequence_numbers`, or `None` when the attempt did not process the batch's own
/// first record and so moved the shard's position not at all.
///
/// A shard has ONE position, so the records it can be read on from past are the run at the
/// HEAD of the batch that were processed: the first record the attempt did not process is
/// the one the next read must start at.
fn checkpointed_through(sequence_numbers: &[String], processed: &[String]) -> Option<String> {
    let consumed: ::std::collections::BTreeSet<&str> =
        processed.iter().map(String::as_str).collect();
    sequence_numbers
        .iter()
        .take_while(|sequence_number| consumed.contains(sequence_number.as_str()))
        .last()
        .cloned()
}

/// Hand one batch to the function, synchronously, as a stream event source's invocation is,
/// and answer with the response document it returned.
async fn invoke_function(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &KinesisSourcedMapping,
    payload: &Value,
) -> Result<Option<Vec<u8>>, String> {
    let answer = clients
        .lambda(callback, &run.region)
        .invoke()
        .function_name(&run.function_arn)
        .payload(::aws_sdk_lambda::primitives::Blob::new(
            payload.to_string().into_bytes(),
        ))
        .send()
        .await
        .map_err(|error| {
            format!(
                "{} could not be invoked ({})",
                run.function_arn,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    // "If present, indicates that an error occurred during function execution"
    // (`InvocationResponse$FunctionError`): the batch was not processed, so it is not
    // checkpointed past.
    if let Some(failure) = answer.function_error.as_deref() {
        return Err(format!(
            "{} answered a {failure} error for the batch",
            run.function_arn
        ));
    }
    // "The response from the function, or an error object" (`InvocationResponse$Payload`):
    // the document a mapping that reads a batch item failure report reads it out of.
    Ok(answer.payload.map(::aws_smithy_types::Blob::into_inner))
}

/// The event one batch is handed over as: the Kinesis record envelopes Lambda builds for a
/// stream event source, one per record, each carrying the record's own data, partition key,
/// sequence number and arrival instant.
fn kinesis_event(
    records: &[::aws_sdk_kinesis::types::Record],
    shard: &str,
    run: &KinesisSourcedMapping,
) -> Value {
    let envelopes: Vec<Value> = records
        .iter()
        .map(|record| {
            let sequence_number = record.sequence_number();
            let mut kinesis = json!({
                "kinesisSchemaVersion": KINESIS_SCHEMA_VERSION,
                "partitionKey": record.partition_key(),
                "sequenceNumber": sequence_number,
                // The record's data travels base64-encoded, as the event a function
                // receives carries it.
                "data": ::base64::engine::general_purpose::STANDARD.encode(record.data().as_ref()),
            });
            if let Some(arrived_at) = record.approximate_arrival_timestamp() {
                kinesis["approximateArrivalTimestamp"] = json!(arrived_at.as_secs_f64());
            }
            let mut envelope = json!({
                "kinesis": kinesis,
                "eventSource": KINESIS_EVENT_SOURCE,
                "eventVersion": KINESIS_EVENT_VERSION,
                // The record is identified by the shard it came from and its own sequence
                // number within that shard.
                "eventID": format!("{shard}:{sequence_number}"),
                "eventName": KINESIS_EVENT_NAME,
                "awsRegion": run.region,
                "eventSourceARN": run.stream_arn,
            });
            if let Some(identity) = run.invoke_identity_arn.as_deref() {
                envelope["invokeIdentityArn"] = json!(identity);
            }
            envelope
        })
        .collect();
    json!({ "Records": envelopes })
}

/// The clients this data plane reaches the other services of this emulator through, built
/// on first use and then reused.
///
/// Each slot remembers the Region its client was built for, because a resource is reached
/// in the Region ITS OWN ARN names.
///
/// Shared with [`super::sqs_poller`], the reader of the other kind of event source, so both
/// pollers reach this emulator the one way.
#[derive(Default)]
pub(crate) struct Clients {
    kinesis: Option<(String, ::aws_sdk_kinesis::Client)>,
    lambda: Option<(String, ::aws_sdk_lambda::Client)>,
}

/// Build each client the same way: over the in-process callback into this same emulator,
/// against the called service's endpoint in `region`.
///
/// The endpoint url names the called service's public host so the request is signed and
/// routed as that service's, but the TRANSPORT is the emulator's own in-process connector
/// ([`smithy_server_runtime::EmbeddedEmulator::http_client`]): no socket is opened and no
/// name is resolved, so a stream or function a mapping named is reached only if this
/// emulator serves it itself.
///
/// The transport is in-process, so each client disables what a network client needs and
/// this one cannot have: retries, timeouts, stalled-stream protection and identity caching
/// each need a sleep implementation the callback transport is not given.
macro_rules! callback_client {
    ($field:ident, $sdk:ident, $endpoint:literal) => {
        pub(crate) fn $field(
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
    callback_client!(kinesis, aws_sdk_kinesis, "kinesis");
    callback_client!(lambda, aws_sdk_lambda, "lambda");
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use ::base64::Engine as _;

    use super::{
        checkpointed_through, iterator_age_millis, kinesis_event, KinesisSourcedMapping,
        MAX_RECORDS_PER_BATCH,
    };
    use crate::state::ShardCursor;

    /// A mapping configuration the envelope rows below are built against.
    fn run() -> KinesisSourcedMapping {
        KinesisSourcedMapping {
            stream_arn: "arn:aws:kinesis:us-east-1:000000000000:stream/orders".to_string(),
            region: "us-east-1".to_string(),
            function_arn: "arn:aws:lambda:us-east-1:000000000000:function:consume".to_string(),
            function_name: "consume".to_string(),
            invoke_identity_arn: Some("arn:aws:iam::000000000000:role/consume".to_string()),
            batch_size: MAX_RECORDS_PER_BATCH,
            reports_batch_item_failures: false,
            cursors: std::collections::BTreeMap::new(),
            initial_cursor: ShardCursor::TrimHorizon,
        }
    }

    #[test]
    fn a_shard_reads_on_from_the_processed_run_at_the_head_of_the_batch() {
        // A shard has ONE position, so what it reads on from is the run of records at the
        // head of the batch the attempt processed. The rows differ along where in the batch
        // the first unprocessed record is: nowhere (every record processed), the head, the
        // middle, the tail, and a batch none of which was processed.
        let batch: Vec<String> = ["49-1", "49-2", "49-3"]
            .iter()
            .map(|sequence_number| (*sequence_number).to_string())
            .collect();
        let rows: [(&str, Vec<&str>, Option<&str>); 5] = [
            (
                "every record processed",
                vec!["49-1", "49-2", "49-3"],
                Some("49-3"),
            ),
            ("the first record failed", vec!["49-2", "49-3"], None),
            ("the middle record failed", vec!["49-1", "49-3"], Some("49-1")),
            ("the last record failed", vec!["49-1", "49-2"], Some("49-2")),
            ("no record processed", Vec::new(), None),
        ];
        for (case, processed, expected) in rows {
            let processed: Vec<String> = processed
                .iter()
                .map(|sequence_number| (*sequence_number).to_string())
                .collect();
            let through = checkpointed_through(&batch, &processed);
            assert_eq!(
                through.as_deref(),
                expected,
                "case {case:?}: a shard whose attempt processed {processed:?} of {batch:?} \
                 must read on from {expected:?}, got {through:?}"
            );
        }
    }

    /// One record as the Kinesis data plane answers it.
    fn record(
        sequence_number: &str,
        data: &[u8],
        arrived_at: Option<SystemTime>,
    ) -> ::aws_sdk_kinesis::types::Record {
        let mut builder = ::aws_sdk_kinesis::types::Record::builder()
            .sequence_number(sequence_number)
            .partition_key("orders")
            .data(::aws_sdk_kinesis::primitives::Blob::new(data.to_vec()));
        if let Some(arrived_at) = arrived_at {
            builder =
                builder.approximate_arrival_timestamp(
                    ::aws_sdk_kinesis::primitives::DateTime::from(arrived_at),
                );
        }
        builder.build().expect("the record is well formed")
    }

    /// RULE: the age one batch meters is how long the NEWEST record of that batch had been
    /// in the shard when the batch was read — never negative, and 0 for a record with no
    /// arrival instant to measure from.
    ///
    /// The rows differ along what the rule generalises over: how long ago the record
    /// arrived (now, a second ago, a minute and a half ago — above one CloudWatch period),
    /// a record whose arrival is ahead of the read, and one carrying no arrival instant at
    /// all.
    #[test]
    fn the_age_a_batch_meters_is_the_wait_of_its_newest_record() {
        let read_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        // (case, when the newest record arrived, the age in milliseconds)
        let rows: Vec<(&str, Option<SystemTime>, f64)> = vec![
            ("read-as-it-arrives", Some(read_at), 0.0),
            (
                "a-second-old",
                Some(read_at - Duration::from_secs(1)),
                1_000.0,
            ),
            (
                "held-past-one-period",
                Some(read_at - Duration::from_secs(90)),
                90_000.0,
            ),
            (
                "arrived-after-the-read",
                Some(read_at + Duration::from_secs(5)),
                0.0,
            ),
            ("no-arrival-instant", None, 0.0),
        ];
        for (case, arrived_at, expected) in rows {
            let record = record("49590338271490256608559692538361571095921575989136588898", b"{}", arrived_at);
            let actual = iterator_age_millis(record.approximate_arrival_timestamp(), read_at);
            assert!(
                (actual - expected).abs() < 1.0,
                "case {case:?}: expected an age of {expected} ms, got {actual:?} ms"
            );
        }
    }

    /// RULE: the event a batch is handed over as carries ONE envelope per record, each with
    /// that record's own data base64-encoded, its partition key, its sequence number and
    /// the stream it came from.
    ///
    /// The rows differ along what the rule generalises over: how many records the batch
    /// holds and what their data is.
    #[test]
    fn a_batch_is_handed_over_as_one_envelope_per_record() {
        let run = run();
        let arrived_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        // (case, the records' data)
        let rows: Vec<(&str, Vec<&[u8]>)> = vec![
            ("one-record", vec![b"first"]),
            ("three-records", vec![b"a", b"bb", b"ccc"]),
            ("an-empty-payload", vec![b""]),
        ];
        for (case, payloads) in rows {
            let records: Vec<::aws_sdk_kinesis::types::Record> = payloads
                .iter()
                .enumerate()
                .map(|(index, data)| record(&format!("4959033827149025660855969253836157109592157598913658889{index}"), data, Some(arrived_at)))
                .collect();
            let event = kinesis_event(&records, "shardId-000000000000", &run);
            let envelopes = event["Records"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                envelopes.len(),
                payloads.len(),
                "case {case:?}: expected {} envelopes, got {envelopes:?}",
                payloads.len()
            );
            for (index, data) in payloads.iter().enumerate() {
                let envelope = &envelopes[index];
                let encoded = ::base64::engine::general_purpose::STANDARD
                    .encode(data)
                    ;
                assert_eq!(
                    envelope["kinesis"]["data"],
                    ::serde_json::json!(encoded),
                    "case {case:?}: record {index} must carry its own data base64-encoded, got {envelope:?}"
                );
                assert_eq!(
                    envelope["eventSourceARN"],
                    ::serde_json::json!(run.stream_arn),
                    "case {case:?}: record {index} must name the stream it came from, got {envelope:?}"
                );
                assert_eq!(
                    envelope["kinesis"]["sequenceNumber"],
                    ::serde_json::json!(records[index].sequence_number()),
                    "case {case:?}: record {index} must carry its own sequence number, got {envelope:?}"
                );
            }
        }
    }
}
