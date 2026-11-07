// SPDX-License-Identifier: CC0-1.0

//! The EventBridge events this service publishes as a job moves through its statuses.
//!
//! # The rule this module implements
//!
//! MediaConvert is an event SOURCE, not only a control plane a client polls: once a job
//! has begun processing, every status it MOVES INTO is published onto the account's
//! default event bus ([`DEFAULT_EVENT_BUS`]) under the `source` this service owns
//! ([`EVENT_SOURCE`]) and the `detail-type` of a job's state change
//! ([`JOB_STATE_CHANGE_DETAIL_TYPE`]), carrying that job's own id and its new status in
//! the event's `detail`. A rule on that bus whose `EventPattern` matches then delivers it
//! to the rule's targets, so a client that reacts to a job through EventBridge observes
//! the run without calling MediaConvert again. `CreateJob$StatusUpdateInterval` — "how
//! often MediaConvert sends STATUS_UPDATE events to Amazon CloudWatch Events" — is the
//! model's own statement that this service publishes a job's progress.
//!
//! A job that never begins is not a run: it publishes nothing. So the status a submit
//! itself stores is not a transition, and neither is the terminal status a cancel writes
//! onto a job still waiting in its queue — a job cancelled before it began processing
//! leaves no event behind.
//!
//! # How a transition becomes an event
//!
//! Nothing here decides WHEN a job moves: the clock does, through
//! [`super::ComAmazonawsMediaconvertMediaconvertState::settle`], and the status it writes
//! is the fact an event reports. The ONE place that writes a job's exit from `SUBMITTED`
//! records the change ([`super::ComAmazonawsMediaconvertMediaconvertState::record_job_state_change`]),
//! so a transition is queued exactly once however it was materialised, and the queue is
//! bounded ([`MAX_PENDING_EVENTS`], oldest dropped first) because every transition appends
//! to it.
//!
//! A queued change is rendered against the job record the store still holds and published
//! with no lock held ([`publish_pending`]): EventBridge is reached the way every emulated
//! service reaches another, through the framework's in-process callback, so this crate
//! names no other service's crate. Publication is best effort — an emulator with no
//! EventBridge mounted has nowhere to publish — so a failure is logged and the operation's
//! own answer stands.
//!
//! WHICH call publishes: the doors whose own answer REPORTS a job's status, or a count
//! derived from it — the job readers, the cancel, the queue readers, the queue delete and
//! the jobs-query and share doors — settle the clock
//! ([`super::ComAmazonawsMediaconvertMediaconvertState::settle`]) and then publish what
//! that settle moved ([`publish_job_state_changes`]), so a transition is published by the
//! very call that materialises it. A drain always empties the WHOLE queue, so a transition
//! a WRITE door's own settle happened to materialise (`CreateJob`, `CreateQueue`,
//! `UpdateQueue`, whose answers report no status the clock moves) is published by the next
//! of those doors rather than lost — which is the same contract this service's clock
//! already has: nothing moves, and so nothing is published, until a door that reads the
//! job lifecycle is called.

use std::time::{SystemTime, UNIX_EPOCH};

use super::{job_arn, queue_arn, ComAmazonawsMediaconvertMediaconvertState, JobRecord};

/// The `source` of every event MediaConvert publishes.
///
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`, and
/// this service's namespace is the one its ARNs carry ([`super::ARN_NAMESPACE`]).
pub const EVENT_SOURCE: &str = "aws.mediaconvert";

/// The `detail-type` of the event published when a JOB changes state.
pub const JOB_STATE_CHANGE_DETAIL_TYPE: &str = "MediaConvert Job State Change";

/// The event bus this service publishes onto.
///
/// The events an AWS service generates go to the account's default event bus, which is the
/// bus a rule carrying no `EventBusName` is created on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// How many job state changes the queue holds at once.
///
/// This module's OWN ceiling, not an AWS quota: the queue lives in memory and every
/// transition appends to it, so without a bound a client submitting without end could grow
/// it without end. The oldest change is dropped first, so the transitions the live jobs
/// just made are the ones that survive. It is far above what a realistic burst produces (a
/// job contributes one change).
pub const MAX_PENDING_EVENTS: usize = 2_000;

/// How many entries one `PutEvents` request carries.
///
/// The bound the service this publishes to declares: its `PutEventsRequestEntryList` is
/// `@length(min: 1, max: 10)`, so a drain holding more changes than that is published in
/// several requests rather than one the service would refuse.
const MAX_PUT_EVENTS_ENTRIES: usize = 10;

/// How many changes ONE drain publishes.
///
/// The queue itself is bounded ([`MAX_PENDING_EVENTS`]), but one call draining all of it
/// would fan out to a rule's targets that many times: this caps one drain, and
/// [`publish_pending`] repeats a drain until the queue is empty, so nothing waits for a
/// later call.
const MAX_PUBLISHED_PER_DRAIN: usize = 50;

/// One job state change waiting to be published.
///
/// Only WHICH job moved, INTO WHICH status and WHEN: the rest of the event is read off the
/// job record when the change is drained, so the event reports the job the store holds
/// rather than a second copy of it.
#[derive(Debug, Clone)]
pub struct JobStateChange {
    /// The job that moved.
    pub job_id: String,
    /// The status it moved INTO, as the record carried it at that instant.
    pub status: String,
    /// WHEN it moved — the instant the clock made the transition due, not the instant some
    /// later call materialised or published it.
    pub at: SystemTime,
}

/// One change rendered as the event that reports it.
#[derive(Debug, Clone)]
struct RenderedEvent {
    /// The job's ARN, which the event envelope carries in `resources`: the event names the
    /// resource it is about, which is what a rule filtering on `resources` reads.
    job_arn: String,
    /// See [`JobStateChange::at`].
    at: SystemTime,
    /// The rendered `detail`.
    detail: ::serde_json::Value,
}

impl ComAmazonawsMediaconvertMediaconvertState {
    /// Queue the transition the job `id` made at `due`, for publication to EventBridge.
    ///
    /// Called from the ONE place a job's exit from `SUBMITTED` is written
    /// ([`Self::settle_job`]), so every transition a processing job makes is recorded
    /// exactly once and none can be published twice. The status a submit STORES is not a
    /// transition and is never recorded here, and neither is the terminal status a cancel
    /// writes onto a job that never began.
    pub(super) fn record_job_state_change(&mut self, id: &str, due: SystemTime) {
        let Some(job) = self.jobs.get(id) else {
            return;
        };
        let change = JobStateChange {
            job_id: job.id.clone(),
            status: job.status.clone(),
            at: due,
        };
        while self.pending_job_events.len() >= MAX_PENDING_EVENTS {
            self.pending_job_events.pop_front();
            ::tracing::warn!(
                "the queue of MediaConvert job state changes waiting to be published to \
                 EventBridge is full at {MAX_PENDING_EVENTS}; the oldest change was dropped"
            );
        }
        self.pending_job_events.push_back(change);
    }

    /// Take at most `limit` of the changes not yet published, oldest first.
    ///
    /// Taking the changes out under the lock is what stops two concurrent calls from
    /// publishing one change twice.
    fn take_job_state_changes(&mut self, limit: usize) -> Vec<JobStateChange> {
        let taking = limit.min(self.pending_job_events.len());
        self.pending_job_events.drain(..taking).collect()
    }

    /// How many changes are queued for publication — reporting only, for a reader that has
    /// to tell "nothing moved" from "nothing was published".
    pub fn pending_job_event_count(&self) -> usize {
        self.pending_job_events.len()
    }
}

/// The `detail` of a job state change: WHICH account's job, in which queue, under which id,
/// the status it moved into and — when the record carries them — why it failed.
///
/// Every member is read off the stored job, so the event reports the job a `GetJob` of the
/// same instant reports and never a second, invented copy of it.
fn job_state_change_detail(
    job: &JobRecord,
    status: &str,
    at: SystemTime,
    region: &str,
) -> ::serde_json::Value {
    let mut detail = ::serde_json::Map::new();
    detail.insert(
        "timestamp".to_string(),
        ::serde_json::Value::from(epoch_millis(at)),
    );
    detail.insert(
        "accountId".to_string(),
        ::serde_json::Value::String(::engine::emulator_account_id().to_string()),
    );
    // The queue is named by its ARN, exactly as `Job$Queue` is projected
    // ([`JobRecord::view`]), so the event and the job agree about which queue ran it.
    detail.insert(
        "queue".to_string(),
        ::serde_json::Value::String(queue_arn(region, &job.queue)),
    );
    detail.insert(
        "jobId".to_string(),
        ::serde_json::Value::String(job.id.clone()),
    );
    detail.insert(
        "status".to_string(),
        ::serde_json::Value::String(status.to_string()),
    );
    // "Error code for the job" / the message describing it: reported exactly when the
    // record carries them, which is the status that failed and no other.
    if let Some(code) = job.error_code {
        detail.insert("errorCode".to_string(), ::serde_json::Value::from(code));
    }
    if let Some(message) = job.error_message.as_ref() {
        detail.insert(
            "errorMessage".to_string(),
            ::serde_json::Value::String(message.clone()),
        );
    }
    // "User-defined metadata that you want to associate with an MediaConvert job": the
    // job's own set, which is what lets a consumer of the event correlate it with whatever
    // submitted the job. An empty set is reported as such.
    detail.insert(
        "userMetadata".to_string(),
        ::serde_json::Value::Object(
            job.user_metadata
                .iter()
                .map(|(key, value)| (key.clone(), ::serde_json::Value::String(value.clone())))
                .collect(),
        ),
    );
    ::serde_json::Value::Object(detail)
}

/// `at` as the epoch milliseconds the event's `time` and its `detail.timestamp` carry.
fn epoch_millis(at: SystemTime) -> i64 {
    i64::try_from(
        at.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or_default()
}

/// Publish every job state change this store has queued onto the account's default
/// EventBridge bus, and leave the queue empty.
///
/// Called with NO lock held: each drain takes its changes out in one critical section, so
/// nothing is published twice, and the in-process `PutEvents` runs outside it. The queue is
/// drained until it is EMPTY — one drain is capped ([`MAX_PUBLISHED_PER_DRAIN`]) so no
/// single call is unbounded, and the loop is what guarantees the tail of a burst is
/// published by the call that produced it. Nothing about the answer the operation gives
/// changes either way.
pub async fn publish_pending<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    engine: &Eng,
    region: &str,
) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let mut events = rendered(&state, region);
    if events.is_empty() {
        return;
    }
    let client = ::service_common::eventbridge_client::eventbridge_client(callback, region);
    while !events.is_empty() {
        publish_batch(&client, &events).await;
        // Whatever a concurrent call left behind: the loop ends on the empty queue, which
        // every drain brings closer because a drain only ever removes from it.
        events = rendered(&state, region);
    }
}

/// One drain of the queue, each change rendered against the job record the store holds.
///
/// A change whose job the store no longer holds renders nothing: an event reports a
/// resource, so there is nothing to report once the resource is gone.
fn rendered(
    state: &::engine::ServiceState<ComAmazonawsMediaconvertMediaconvertState>,
    region: &str,
) -> Vec<RenderedEvent> {
    let mut store = state.lock();
    let changes = store.take_job_state_changes(MAX_PUBLISHED_PER_DRAIN);
    changes
        .into_iter()
        .filter_map(|change| {
            let job = store.jobs.get(&change.job_id)?;
            Some(RenderedEvent {
                job_arn: job_arn(region, &job.id),
                at: change.at,
                detail: job_state_change_detail(job, &change.status, change.at, region),
            })
        })
        .collect()
}

/// Publish one drain, in `PutEvents` requests of at most [`MAX_PUT_EVENTS_ENTRIES`]
/// entries.
async fn publish_batch(client: &::aws_sdk_eventbridge::Client, events: &[RenderedEvent]) {
    for window in events.chunks(MAX_PUT_EVENTS_ENTRIES) {
        let mut request = client.put_events();
        for event in window {
            let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                .event_bus_name(DEFAULT_EVENT_BUS)
                .source(EVENT_SOURCE)
                .detail_type(JOB_STATE_CHANGE_DETAIL_TYPE)
                // The event's `time` is WHEN the job moved, not when the queue was
                // drained: `PutEventsRequestEntry$Time` is "the time stamp of the event,
                // per RFC3339. If no time stamp is provided, the time stamp of the
                // PutEvents call is used."
                .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                    epoch_millis(event.at),
                ))
                .resources(&event.job_arn)
                .detail(event.detail.to_string())
                .build();
            request = request.entries(entry);
        }
        if let Err(error) = request.send().await {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a MediaConvert job state change could not be published to EventBridge"
            );
        }
    }
}

/// Publish what THIS call materialised, for a door that settled the clock itself.
///
/// The drain [`publish_pending`] makes, reached from inside an operation: called after the
/// door has released the store's lock, so no lock is held across the in-process call it
/// makes. A call that materialised no transition publishes nothing, so this costs one
/// probe of an empty queue on the doors that moved nothing.
pub async fn publish_job_state_changes<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &Eng,
    region: &str,
) {
    publish_pending(ctx.callback(), engine, region).await;
}
