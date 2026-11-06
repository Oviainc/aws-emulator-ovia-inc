// SPDX-License-Identifier: CC0-1.0

//! The EventBridge events this service publishes as an execution moves: one event per
//! state change of the execution, of each of its stages and of each of their actions.
//!
//! # The rule this module implements
//!
//! CodePipeline does not only record an execution's progress for a reader to poll: every
//! state change of an execution, of a stage of it and of an action of that stage is
//! PUBLISHED onto the account's default EventBridge bus, under the `source` this service
//! owns ([`EVENT_SOURCE`]) and the `detail-type` of the thing that moved
//! ([`PIPELINE_EXECUTION_STATE_CHANGE`], [`STAGE_EXECUTION_STATE_CHANGE`],
//! [`ACTION_EXECUTION_STATE_CHANGE`]), carrying the resource that moved and the state it
//! moved into in the event's `detail`. A rule on that bus whose `EventPattern` matches
//! then delivers it to that rule's targets, so a client that reacts to a pipeline through
//! EventBridge observes the run without calling CodePipeline again.
//!
//! # How a change becomes an event
//!
//! Nothing here decides WHEN anything moves: the engine ([`super::driver`]) and the clock
//! ([`super::clock`]) do, and a status they write is the fact an event reports. So this
//! module is a DIFF rather than a second state machine: [`record_state_changes`] compares
//! every stored execution against the states last published for it
//! ([`PublishedStates`], held in the store beside the executions) and queues one change
//! per status that differs — whichever call, read or clock tick materialised it, and
//! without any transition site having to remember to announce itself.
//!
//! A queued change is published by [`publish`], which runs with no lock held: EventBridge
//! is reached the way every emulated service reaches another, through the framework's
//! in-process callback, so this crate names no other service's crate. Publication is
//! best-effort — an emulator with no EventBridge mounted has nowhere to publish — so a
//! failure is logged and the operation's own result stands.
//!
//! The queue is BOUNDED ([`MAX_PENDING_EVENTS`], oldest dropped first) because every
//! transition appends to it, and the snapshot map is pruned to the executions the store
//! still holds, so neither grows with the number of calls served.

use std::collections::VecDeque;
use std::time::SystemTime;

use super::execution::{declared_action, is_terminal_status};
use super::ComAmazonawsCodepipelineCodepipeline20150709State;

/// The `source` of every event CodePipeline publishes.
///
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`,
/// and this service's namespace is the one its ARNs carry ([`super::ARN_SERVICE`]).
pub const EVENT_SOURCE: &str = "aws.codepipeline";

/// The `detail-type` of the event published when a PIPELINE EXECUTION changes state.
pub const PIPELINE_EXECUTION_STATE_CHANGE: &str = "CodePipeline Pipeline Execution State Change";

/// The `detail-type` of the event published when one STAGE of an execution changes state.
pub const STAGE_EXECUTION_STATE_CHANGE: &str = "CodePipeline Stage Execution State Change";

/// The `detail-type` of the event published when one ACTION of a stage changes state.
pub const ACTION_EXECUTION_STATE_CHANGE: &str = "CodePipeline Action Execution State Change";

/// The event bus this service publishes onto.
///
/// The events an AWS service generates go to the account's default event bus, which is
/// the bus a rule carrying no `EventBusName` is created on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// The event `state` of a resource that has just begun running.
const STATE_STARTED: &str = "STARTED";
/// The event `state` of a resource that is running again after it had finished.
const STATE_RESUMED: &str = "RESUMED";
/// See [`STATE_STARTED`].
const STATE_SUCCEEDED: &str = "SUCCEEDED";
/// See [`STATE_STARTED`].
const STATE_FAILED: &str = "FAILED";
/// See [`STATE_STARTED`]. Spelled with one `l`, as the event schema spells it, which the
/// modelled `Cancelled` status is mapped onto rather than upper-cased into.
const STATE_CANCELED: &str = "CANCELED";
/// See [`STATE_STARTED`].
const STATE_STOPPING: &str = "STOPPING";
/// See [`STATE_STARTED`].
const STATE_STOPPED: &str = "STOPPED";
/// See [`STATE_STARTED`].
const STATE_SUPERSEDED: &str = "SUPERSEDED";
/// See [`STATE_STARTED`].
const STATE_ABANDONED: &str = "ABANDONED";

/// How many changes the queue holds at once.
///
/// This module's OWN ceiling, not an AWS quota: the queue lives in memory and every
/// transition appends to it, so without a bound a client looping on starts could grow it
/// without end. The oldest change is dropped first, so the transitions a live execution
/// just made are the ones that survive. It is far above what a realistic burst produces
/// (an execution contributes a handful of changes per stage).
pub const MAX_PENDING_EVENTS: usize = 2_000;

/// How many entries one `PutEvents` request carries.
///
/// The bound the service this publishes to declares: its `PutEventsRequestEntryList` is
/// `@length(min: 1, max: 10)`, so a batch holding more changes than that is published in
/// several requests rather than one the service would refuse.
const MAX_PUT_EVENTS_ENTRIES: usize = 10;

/// How many changes ONE batch of a drain publishes.
///
/// The queue itself is bounded ([`MAX_PENDING_EVENTS`]), but one request dispatching all
/// of it would fan out to a rule's targets that many times: this caps one batch, and
/// [`publish`] repeats a batch until the queue is empty, so nothing waits for a later
/// call.
const MAX_PUBLISHED_PER_DRAIN: usize = 50;

/// One state change waiting to be published.
///
/// The `detail` is rendered when the change is RECORDED, so the event reports the
/// resource as it was at that instant rather than as a later drain finds it.
#[derive(Debug, Clone)]
pub struct StateChange {
    /// The `detail-type` the event carries.
    pub detail_type: &'static str,
    /// The pipeline's ARN, which the event envelope carries in `resources`: the event
    /// names the resource it is about, which is what a rule filtering on `resources`
    /// reads.
    pub pipeline_arn: String,
    /// WHEN the resource moved, in epoch milliseconds — the instant this call
    /// materialised the transition, the same instant the store's own write carries.
    pub at: i64,
    /// The rendered `detail` of the event.
    pub detail: ::serde_json::Value,
}

/// The states last PUBLISHED for one execution, against which the next diff is taken.
///
/// Stage and action states are held positionally, in the execution's own plan order, so a
/// stage of a plan that holds only some of the pipeline's stages (a rollback runs one) is
/// still compared against its own previous state.
#[derive(Debug, Default, Clone)]
pub struct PublishedStates {
    /// The `PipelineExecutionStatus` last published for the execution.
    execution: Option<String>,
    /// The `StageExecutionStatus` last published for each stage of the plan.
    stages: Vec<Option<String>>,
    /// The `ActionExecutionStatus` last published for each action of each stage.
    actions: Vec<Vec<Option<String>>>,
}

/// Queue one event for every stored status that differs from the state last published for
/// it, and take that state to be the published one.
///
/// This is the whole of the "which transitions owe an event" rule: a status the engine
/// wrote is compared against the last one published, so every move — an execution
/// entering a stage, an action finishing, a stop settling, a supersession — owes exactly
/// one event, and a status that has not changed owes none however many times this runs.
///
/// `region` is the Region the execution runs in, which an action event reports; `now` is
/// the instant the transitions being recorded were materialised at.
pub(super) fn record_state_changes(
    store: &mut ComAmazonawsCodepipelineCodepipeline20150709State,
    region: &str,
    now: SystemTime,
) {
    // The pipelines, the executions, the snapshots and the queue are DISJOINT fields of
    // the store, so a pinned declaration is read while the snapshot and the queue are
    // written — no clone of a declaration is made per change.
    let ComAmazonawsCodepipelineCodepipeline20150709State {
        pipelines,
        executions,
        published_event_states: published,
        pending_events: pending,
        ..
    } = store;
    let at = epoch_millis(now);
    for (id, record) in executions.iter() {
        let Some(pipeline) = pipelines.get(&record.pipeline_name) else {
            continue;
        };
        let declaration = pipeline.declaration_at(Some(record.pipeline_version));
        let snapshot = published.entry(id.clone()).or_default();
        // The execution's own change, and whether it is the execution BEGINNING: see
        // where it is queued below.
        let mut execution_change: Option<(bool, StateChange)> = None;

        if snapshot.execution.as_deref() != Some(record.status.as_str()) {
            let state = execution_event_state(snapshot.execution.as_deref(), &record.status);
            snapshot.execution = Some(record.status.clone());
            if let Some(state) = state {
                execution_change = Some((
                    matches!(state, STATE_STARTED | STATE_RESUMED),
                    StateChange {
                        detail_type: PIPELINE_EXECUTION_STATE_CHANGE,
                        pipeline_arn: pipeline.arn.clone(),
                        at,
                        detail: execution_detail(record, state),
                    },
                ));
            }
        }
        // An execution BEGINS before the stage it enters and FINISHES after the stage
        // whose outcome decided it, and one materialised step may write both: the
        // execution's own event is queued before its stages' when it begins and after
        // them otherwise, so the order the events reach a target is the order in which
        // the things they report happened.
        if execution_change
            .as_ref()
            .is_some_and(|(begins, _)| *begins)
        {
            if let Some((_, change)) = execution_change.take() {
                enqueue(pending, change);
            }
        }

        // One slot per stage of the plan, and per action of each stage: the snapshot is a
        // positional view of this execution's own plan, so it is grown to the plan's
        // shape before it is compared against it.
        snapshot.stages.resize(record.stages.len(), None);
        snapshot.actions.resize(record.stages.len(), Vec::new());
        for (index, stage) in record.stages.iter().enumerate() {
            // Both slots exist: the resize above gave the snapshot the plan's shape, and
            // each is reached through `get_mut` rather than an index, so a plan and a
            // snapshot that ever disagreed would skip the stage instead of panicking.
            if let Some(previous) = snapshot.stages.get_mut(index) {
                if previous.as_deref() != stage.status.as_deref() {
                    let state = stage
                        .status
                        .as_deref()
                        .and_then(|status| stage_event_state(previous.as_deref(), status));
                    *previous = stage.status.clone();
                    if let Some(state) = state {
                        enqueue(
                            pending,
                            StateChange {
                                detail_type: STAGE_EXECUTION_STATE_CHANGE,
                                pipeline_arn: pipeline.arn.clone(),
                                at,
                                detail: stage_detail(record, &stage.name, state),
                            },
                        );
                    }
                }
            }
            let Some(actions) = snapshot.actions.get_mut(index) else {
                continue;
            };
            actions.resize(stage.actions.len(), None);
            for (position, action) in stage.actions.iter().enumerate() {
                let Some(previous) = actions.get_mut(position) else {
                    continue;
                };
                if previous.as_deref() == action.status.as_deref() {
                    continue;
                }
                let state = action.status.as_deref().and_then(action_event_state);
                *previous = action.status.clone();
                if let Some(state) = state {
                    enqueue(
                        pending,
                        StateChange {
                            detail_type: ACTION_EXECUTION_STATE_CHANGE,
                            pipeline_arn: pipeline.arn.clone(),
                            at,
                            detail: action_detail(
                                record,
                                &stage.name,
                                action,
                                declaration
                                    .and_then(|declaration| {
                                        declared_action(declaration, stage, position)
                                    })
                                    .map(|declared| &declared.action_type_id),
                                region,
                                state,
                            ),
                        },
                    );
                }
            }
        }
        // The execution FINISHED (or is stopping): its event follows the stage and action
        // events of the same step.
        if let Some((_, change)) = execution_change {
            enqueue(pending, change);
        }
    }
    // A snapshot belongs to an execution: one the store no longer holds (the history
    // bound dropped it, or its pipeline was deleted) can never differ again.
    published.retain(|id, _| executions.contains_key(id));
}

/// `time` as the epoch milliseconds an event's `time` carries.
///
/// The same conversion [`super::to_date_time`] makes for a modelled timestamp member,
/// answered as the figure the event client takes.
fn epoch_millis(time: SystemTime) -> i64 {
    super::to_date_time(time).to_millis().unwrap_or_default()
}

/// Append `change` to the queue, dropping the oldest once the queue is full.
fn enqueue(pending: &mut VecDeque<StateChange>, change: StateChange) {
    while pending.len() >= MAX_PENDING_EVENTS {
        pending.pop_front();
        ::tracing::warn!(
            "the queue of pipeline state changes waiting to be published to EventBridge is \
             full at {MAX_PENDING_EVENTS}; the oldest change was dropped"
        );
    }
    pending.push_back(change);
}

/// The event `state` an execution MOVING INTO `status` reports, or `None` for a status the
/// event schema states no event for.
///
/// An execution running again after it had already finished is a RESUME rather than a
/// start, which is what a retry of a failed stage produces.
fn execution_event_state(previous: Option<&str>, status: &str) -> Option<&'static str> {
    match status {
        "InProgress" => Some(match previous.is_some_and(is_terminal_status) {
            true => STATE_RESUMED,
            false => STATE_STARTED,
        }),
        "Succeeded" => Some(STATE_SUCCEEDED),
        "Failed" => Some(STATE_FAILED),
        "Stopping" => Some(STATE_STOPPING),
        "Stopped" => Some(STATE_STOPPED),
        "Superseded" => Some(STATE_SUPERSEDED),
        "Cancelled" => Some(STATE_CANCELED),
        _ => None,
    }
}

/// The event `state` a STAGE moving into `status` reports.
///
/// `Skipped` — the status of a stage an entry condition's `SKIP` result kept the execution
/// out of — is the one modelled `StageExecutionStatus` the event schema names no state
/// for, so nothing is published for it rather than a state name being invented.
fn stage_event_state(previous: Option<&str>, status: &str) -> Option<&'static str> {
    match status {
        "InProgress" => Some(match previous.is_some_and(is_terminal_stage_status) {
            true => STATE_RESUMED,
            false => STATE_STARTED,
        }),
        "Succeeded" => Some(STATE_SUCCEEDED),
        "Failed" => Some(STATE_FAILED),
        "Stopping" => Some(STATE_STOPPING),
        "Stopped" => Some(STATE_STOPPED),
        "Cancelled" => Some(STATE_CANCELED),
        _ => None,
    }
}

/// True for a `StageExecutionStatus` a stage's run can no longer leave on its own, so a
/// move back into `InProgress` from it is a resume.
fn is_terminal_stage_status(status: &str) -> bool {
    matches!(
        status,
        "Succeeded" | "Failed" | "Stopped" | "Cancelled" | "Skipped"
    )
}

/// The event `state` an ACTION moving into `status` reports.
fn action_event_state(status: &str) -> Option<&'static str> {
    match status {
        "InProgress" => Some(STATE_STARTED),
        "Succeeded" => Some(STATE_SUCCEEDED),
        "Failed" => Some(STATE_FAILED),
        "Abandoned" => Some(STATE_ABANDONED),
        _ => None,
    }
}

/// The `detail` of a pipeline execution state change: which pipeline, which execution,
/// the pipeline version the execution runs, what started it, and the state it moved into.
fn execution_detail(
    record: &super::PipelineExecutionRecord,
    state: &'static str,
) -> ::serde_json::Value {
    let mut detail = ::serde_json::Map::new();
    detail.insert(
        "pipeline".to_string(),
        ::serde_json::Value::String(record.pipeline_name.clone()),
    );
    detail.insert(
        "execution-id".to_string(),
        ::serde_json::Value::String(record.pipeline_execution_id.clone()),
    );
    detail.insert("state".to_string(), ::serde_json::Value::String(state.to_string()));
    detail.insert(
        "version".to_string(),
        ::serde_json::Value::from(record.pipeline_version),
    );
    let mut trigger = ::serde_json::Map::new();
    if let Some(kind) = record.trigger.trigger_type.as_ref() {
        trigger.insert(
            "trigger-type".to_string(),
            ::serde_json::Value::String(kind.clone()),
        );
    }
    if let Some(what) = record.trigger.trigger_detail.as_ref() {
        trigger.insert(
            "trigger-detail".to_string(),
            ::serde_json::Value::String(what.clone()),
        );
    }
    // An execution nothing recorded a trigger for carries no `execution-trigger`, rather
    // than an empty object standing for a trigger.
    if !trigger.is_empty() {
        detail.insert(
            "execution-trigger".to_string(),
            ::serde_json::Value::Object(trigger),
        );
    }
    ::serde_json::Value::Object(detail)
}

/// The `detail` of a stage execution state change: the execution's own members plus the
/// stage that moved.
fn stage_detail(
    record: &super::PipelineExecutionRecord,
    stage: &str,
    state: &'static str,
) -> ::serde_json::Value {
    let mut detail = ::serde_json::Map::new();
    detail.insert(
        "pipeline".to_string(),
        ::serde_json::Value::String(record.pipeline_name.clone()),
    );
    detail.insert(
        "execution-id".to_string(),
        ::serde_json::Value::String(record.pipeline_execution_id.clone()),
    );
    detail.insert(
        "stage".to_string(),
        ::serde_json::Value::String(stage.to_string()),
    );
    detail.insert("state".to_string(), ::serde_json::Value::String(state.to_string()));
    detail.insert(
        "version".to_string(),
        ::serde_json::Value::from(record.pipeline_version),
    );
    ::serde_json::Value::Object(detail)
}

/// The `detail` of an action execution state change: the stage's members plus the action
/// that moved, the id its run is reported under, the Region it ran in and the action
/// TYPE the declaration gives it.
fn action_detail(
    record: &super::PipelineExecutionRecord,
    stage: &str,
    action: &super::ActionRun,
    action_type: Option<&crate::ops::types::ActionTypeId>,
    region: &str,
    state: &'static str,
) -> ::serde_json::Value {
    let mut detail = ::serde_json::Map::new();
    detail.insert(
        "pipeline".to_string(),
        ::serde_json::Value::String(record.pipeline_name.clone()),
    );
    detail.insert(
        "execution-id".to_string(),
        ::serde_json::Value::String(record.pipeline_execution_id.clone()),
    );
    detail.insert(
        "stage".to_string(),
        ::serde_json::Value::String(stage.to_string()),
    );
    detail.insert(
        "action".to_string(),
        ::serde_json::Value::String(action.name.clone()),
    );
    detail.insert("state".to_string(), ::serde_json::Value::String(state.to_string()));
    detail.insert(
        "action-execution-id".to_string(),
        ::serde_json::Value::String(action.action_execution_id.clone()),
    );
    detail.insert(
        "region".to_string(),
        ::serde_json::Value::String(region.to_string()),
    );
    detail.insert(
        "version".to_string(),
        ::serde_json::Value::from(record.pipeline_version),
    );
    // The action's type is the declaration's, so an action whose pinned declaration no
    // longer declares it reports no type rather than a guessed one.
    if let Some(action_type) = action_type {
        detail.insert(
            "type".to_string(),
            ::serde_json::json!({
                "owner": action_type.owner,
                "category": action_type.category,
                "provider": action_type.provider,
                "version": action_type.version,
            }),
        );
    }
    ::serde_json::Value::Object(detail)
}

/// Publish every state change this store has queued onto the account's default
/// EventBridge bus, and leave the queue empty.
///
/// Called with NO lock held: each batch of changes is taken out in one critical section,
/// so nothing is published twice, and the in-process `PutEvents` runs outside it. The
/// queue is drained until it is EMPTY — a batch is capped
/// ([`MAX_PUBLISHED_PER_DRAIN`]) so no single request is unbounded, and the loop is what
/// guarantees the tail of a burst is published by the call that produced it rather than
/// waiting for the next one. Nothing about the answer changes what the caller of the
/// operation is told — an emulator with no EventBridge mounted has nowhere to publish —
/// so a failure is logged and the operation's own result stands.
pub(super) async fn publish(
    state: &::engine::ServiceState<ComAmazonawsCodepipelineCodepipeline20150709State>,
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) {
    // The guard is taken and dropped inside this statement.
    let mut changes = take_pending(&mut state.lock());
    if changes.is_empty() {
        return;
    }
    let client = events_client(callback, region);
    while !changes.is_empty() {
        publish_batch(&client, &changes).await;
        // What the transitions published above queued in turn — nothing does, today —
        // and whatever a concurrent call left behind: the loop ends on the empty queue,
        // which every batch brings closer because a batch is only ever removed from it.
        changes = take_pending(&mut state.lock());
    }
}

/// Publish one batch of changes, in `PutEvents` requests of at most
/// [`MAX_PUT_EVENTS_ENTRIES`] entries.
async fn publish_batch(client: &::aws_sdk_eventbridge::Client, changes: &[StateChange]) {
    for window in changes.chunks(MAX_PUT_EVENTS_ENTRIES) {
        let mut request = client.put_events();
        for change in window {
            let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                .event_bus_name(DEFAULT_EVENT_BUS)
                .source(EVENT_SOURCE)
                .detail_type(change.detail_type)
                // The event's `time` is WHEN the resource moved, not when the queue was
                // drained: `PutEventsRequestEntry$Time` is "the time stamp of the event,
                // per RFC3339. If no time stamp is provided, the time stamp of the
                // PutEvents call is used."
                .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                    change.at,
                ))
                .resources(&change.pipeline_arn)
                .detail(change.detail.to_string())
                .build();
            request = request.entries(entry);
        }
        if let Err(error) = request.send().await {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a CodePipeline state change could not be published to EventBridge"
            );
        }
    }
}

/// Take at most [`MAX_PUBLISHED_PER_DRAIN`] queued changes out of the store, oldest
/// first — one batch of the drain [`publish`] repeats until the queue is empty.
fn take_pending(
    store: &mut ComAmazonawsCodepipelineCodepipeline20150709State,
) -> Vec<StateChange> {
    let taking = MAX_PUBLISHED_PER_DRAIN.min(store.pending_events.len());
    store.pending_events.drain(..taking).collect()
}

/// An EventBridge client over the in-process callback into this same emulator.
///
/// Built exactly like the clients the driver performs an action's work with
/// ([`super::driver`]): a fixed endpoint literal ([`super::in_process_endpoint`]), the
/// emulator's own credentials, and no retry, timeout or stalled-stream machinery, because
/// the transport is in process.
fn events_client(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_eventbridge::Client {
    let config = ::aws_sdk_eventbridge::Config::builder()
        .behavior_version(::aws_sdk_eventbridge::config::BehaviorVersion::latest())
        .region(::aws_sdk_eventbridge::config::Region::new(region.to_string()))
        .endpoint_url(super::in_process_endpoint("events"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_eventbridge::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_eventbridge::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_eventbridge::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_eventbridge::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_eventbridge::Client::from_conf(config)
}

