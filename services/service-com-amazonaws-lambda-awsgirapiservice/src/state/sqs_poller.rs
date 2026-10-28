// SPDX-License-Identifier: CC0-1.0

//! The POLLER behind an event source mapping whose event source is an Amazon SQS queue.
//!
//! `CreateEventSourceMapping` "creates a mapping between an event source and an Lambda
//! Event Function. Lambda reads items from the event source and invokes the function", and
//! `Enabled` is "when true, the event source mapping is active. When false, Lambda pauses
//! polling and invocation" — so a mapping that reports `Enabled` is one that READS from its
//! source. That reading is what this module does: while such a mapping exists, its queue is
//! polled, a batch of at most `BatchSize` messages is taken out of the queue the mapping
//! names and handed to the mapping's function, and the mapping records the result of that
//! attempt.
//!
//! The batch is taken through the QUEUE's own delivery bookkeeping
//! (`service_com_amazonaws_sqs_amazonsqs::state::Queue::hand_over`), the one path every
//! consumer of a queue goes through, so a delivery the poller makes counts exactly as a
//! client's `ReceiveMessage` does: the message's `ApproximateReceiveCount` rises, it is
//! hidden for the queue's `VisibilityTimeout` and it becomes visible again when that
//! lapses. A poll also applies what the queue's own `RedrivePolicy` owes first, so a
//! message that has had every delivery that policy allows is moved to the dead-letter queue
//! the policy names instead of being handed over again.
//!
//! What this emulator cannot do is RUN the batch: there is no in-process runtime for a
//! function's deployment package (the same limitation `Invoke` answers 501 for). So the
//! batch is handed over and nothing processes it, which is precisely the case a queue's
//! redrive policy exists for — the consumer never deletes the batch, so every message in it
//! comes back when its visibility timeout lapses and is eventually parked in the
//! dead-letter queue. The mapping reports that outcome as its `LastProcessingResult` rather
//! than a success nothing earned, and nothing here ever deletes a message the function did
//! not process.
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
//! wake-up's work and takes the instant it is working at — can be called directly instead.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ::service_com_amazonaws_sqs_amazonsqs as sqs;

use super::{
    service_state, ComAmazonawsLambdaAwsgirapiserviceState, EVENT_SOURCE_MAPPING_STATE_ENABLED,
    EVENT_SOURCE_VENDOR_QUEUE, STATE_ACTIVE,
};

/// How long the poll loop waits between two polls of a mapping's queue.
///
/// An EMULATOR-CHOSEN span, like the other spans this crate owns: Lambda publishes no poll
/// frequency for an event source mapping, only that the mapping polls while it is enabled.
/// It is deliberately non-zero, so the loop hands the runtime back between wake-ups instead
/// of spinning, and short enough that a caller watching its queue sees the mapping consume.
pub const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// What a mapping reports as its `LastProcessingResult` — "the result of the event source
/// mapping's last processing attempt" — after a poll handed a batch to its function.
///
/// The batch was delivered and nothing ran it, because this emulator has no in-process
/// runtime for a deployment package, so that is what the attempt's result says, in the
/// `PROBLEM: <reason>` form Lambda words an unsuccessful attempt in — short and stable,
/// because a client reads it on every mapping it fetches, with the whole account of why in
/// this module's own documentation. The reason itself is this emulator's own: no AWS run
/// produces it, because no AWS poller has a deployment package it cannot run. The batch is
/// left on the queue, exactly as an unprocessed batch is.
pub const BATCH_NOT_PROCESSED_RESULT: &str =
    "PROBLEM: the function's deployment package cannot be run by this emulator";

/// One mapping's queue, as the poll loop reads it out of the store before it touches a
/// queue: the mapping to record the attempt against, the queue to poll and how many
/// messages one batch may hold.
struct PollTarget {
    /// `UUID`, which is the key the mapping is stored under.
    uuid: String,
    /// `EventSourceArn`, the queue to take the batch out of.
    queue_arn: String,
    /// `BatchSize`: "the maximum number of records in each batch that Lambda pulls from
    /// your stream or queue".
    batch_size: usize,
}

/// Start the background poll loop for this engine's mappings, if one is not already running.
///
/// Called by the operations that make or change a mapping. At most one loop runs per store
/// (the store carries the claim), it polls for as long as the store holds a mapping that
/// wants polling and stands down when it does not, and with no async runtime to spawn onto
/// there is no loop — a caller can then drive the same work through [`poll_due`].
pub fn drive_sqs_pollers<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = claim_poller(&mut state.lock()) else {
        return;
    };
    let engine = Arc::clone(engine);
    let claimed = ::engine::ServiceState::clone(&state);
    let handle = runtime.spawn(async move {
        // The loop ends when nothing polls any more, which it observes — and releases its
        // claim in — inside one critical section.
        while polling_wanted_or_release(&mut state.lock(), generation) {
            // Every wake-up waits, which is what hands the runtime back between polls. No
            // lock is held across it.
            ::tokio::time::sleep(POLL_INTERVAL).await;
            poll_due(&*engine, SystemTime::now());
        }
    });
    // The handle is kept so the loop it replaces is aborted rather than merely forgotten.
    set_poller(&mut claimed.lock(), generation, handle);
}

/// One wake-up's work: poll every enabled queue mapping this engine holds, at `now`.
///
/// For each such mapping, in turn: whatever the clock already owed on the queues is applied,
/// the queue's `RedrivePolicy` is applied to the messages that have had every delivery it
/// allows, and then a batch of at most the mapping's `BatchSize` messages is handed over.
/// Nothing is deleted — see this module's documentation for why an undeleted batch is the
/// honest outcome here.
pub fn poll_due<Eng: Send + Sync + 'static>(engine: &Eng, now: SystemTime) {
    let Some(mappings) = service_state(engine) else {
        return;
    };
    let Some(queues) = sqs::state::service_state(engine) else {
        return;
    };
    // The two stores are never locked at the same time: each step takes one lock, finishes
    // with it and releases it before the next step takes the other.
    let targets = {
        let mut store = mappings.lock();
        store.materialize_all(now);
        polling_targets(&store)
    };
    for target in targets {
        let handed = {
            let mut store = queues.lock();
            // Retention lapses, visibility timeouts and message movement tasks are owed by
            // the clock, so what the delivery below sees is what the queue may hand out.
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
                // The mapping names a queue this engine no longer holds, so there is
                // nothing to poll.
                continue;
            };
            // A message this queue has already handed over as many times as its
            // `RedrivePolicy` allows is not handed over again: it goes to the dead-letter
            // queue that policy names, BEFORE this poll decides what there is to deliver.
            store.dead_letter_due(&region, &name, now);
            let Some(queue) = store.queue_by_arn_mut(&target.queue_arn) else {
                continue;
            };
            // The batch is hidden for the queue's own `VisibilityTimeout`, which is the span
            // a message is handed to ONE consumer for, and which `CreateEventSourceMapping`
            // already held this pair to.
            queue.hand_over(
                now,
                target.batch_size,
                sqs::state::after(now, visibility_timeout),
            )
        };
        if handed.is_empty() {
            // Nothing was waiting, so no processing attempt was made and there is no result
            // of one to report.
            continue;
        }
        if let Some(mapping) = mappings.lock().event_source_mappings.get_mut(&target.uuid) {
            mapping.last_processing_result = Some(BATCH_NOT_PROCESSED_RESULT.to_string());
        }
    }
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
            Some(PollTarget {
                uuid: mapping.uuid.clone(),
                queue_arn: queue_event_source(mapping.event_source_arn.as_deref())?.to_string(),
                // "Amazon Simple Queue Service – Default 10. For standard queues the max is
                // 10,000": the size is the batch's ceiling, and a mapping always pulls at
                // least one record.
                batch_size: usize::try_from(mapping.batch_size).unwrap_or(1).max(1),
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
