// SPDX-License-Identifier: CC0-1.0

//! The in-process CLOCK that carries a started execution through its stages without a
//! further call from the client.
//!
//! A client that starts a pipeline execution then watches the artifact store for the
//! deployed object: it makes no second CodePipeline call, so every step the clock makes
//! due after the start — entering the next stage, running the next action's work — would
//! never be materialised at all if only the next request could perform it. This is the
//! one task that waits for those instants, so no request ever has to.
//!
//! It does not change WHEN a step happens. The task calls exactly the same
//! [`super::driver::advance_pipeline`] every handler calls, which materialises only the
//! steps whose stored due instant has already passed ([`super::execution`]), so a caller
//! reading an execution before that instant still sees the transient state. The clock
//! only removes the need for a caller to be the one that asks.
//!
//! At most one clock runs per store (the store carries the claim, which a clock whose
//! task has ended without releasing it does not keep — [`claim`]), each clock is bounded
//! by [`MAX_WAKE_UPS`] wake-ups, and with no async runtime to spawn onto there is no
//! clock: the steps are then materialised by the next operation instead, as they always
//! were.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use super::execution::is_terminal_status;
use super::ComAmazonawsCodepipelineCodepipeline20150709State;

/// The shortest a wake-up waits, so a step already past its due instant costs a yield
/// rather than a CPU-bound spin.
const MIN_TICK: Duration = Duration::from_millis(25);

/// The longest a wake-up waits, so the clock re-reads the store — which a request may
/// have changed since — at least this often.
const MAX_TICK: Duration = Duration::from_secs(2);

/// How many times the wake-up floor doubles while nothing moves.
///
/// An execution can be parked on something only a REQUEST can release (a manual
/// approval, a disabled transition, a custom action's job worker). Nothing is due for it
/// until that request arrives — and that request advances the execution itself — so a
/// clock that finds no work backs off to `MIN_TICK << MAX_IDLE_SHIFT` between wake-ups
/// instead of polling a parked store at full rate. The floor is reset by the first
/// wake-up that performs work again.
const MAX_IDLE_SHIFT: u32 = 5;

/// How many wake-ups that find NOTHING to do one clock takes before it stands down.
///
/// A backstop, not a schedule: it bounds how long one request's clock keeps polling a
/// store that is parked on something only another request can release. A clock that is
/// still materialising steps is not spent — every wake-up that performs work resets the
/// count — so an execution in flight is carried to its terminal status rather than
/// abandoned part way through. Whatever a spent clock has not reached is materialised by
/// the next clock (every call that advances a pipeline starts one if none is running) or
/// by the next call itself.
const MAX_IDLE_WAKE_UPS: usize = 1_024;

/// Start the one in-process task that waits for this store's earliest due execution
/// step, materialises what is then due, and stands down once no execution of any
/// pipeline is still in flight.
///
/// Called by [`super::driver::advance`], so every operation that advances a pipeline
/// keeps the clock running; a second call while one runs starts nothing.
pub(super) fn drive(
    state: &::engine::ServiceState<ComAmazonawsCodepipelineCodepipeline20150709State>,
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) {
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = claim(&mut state.lock()) else {
        return;
    };
    let driven = ::engine::ServiceState::clone(state);
    // The same handle into this same emulator the request itself would have performed
    // the action's work through, owned by the task rather than borrowed from the request
    // that started it. The Region is that request's too: it names which endpoint of this
    // same in-process emulator the action's S3, CodeCommit and Lambda calls go to, and
    // WHICH bucket and object each of those calls names is already on the execution
    // itself (the artifact store it pinned at start), so it is not re-derived here.
    let callback = callback.clone();
    let region = region.to_string();
    let handle = runtime.spawn(async move {
        // `true` only if the idle cap is what ended the loop; a clock that ran out of
        // work has already released its claim inside the one critical section that
        // observed it.
        let mut capped = true;
        let mut idle_shift = 0;
        let mut idle_wake_ups = 0;
        while idle_wake_ups < MAX_IDLE_WAKE_UPS {
            let Some(pending) = pending_or_release(&mut driven.lock(), generation) else {
                capped = false;
                break;
            };
            // Every wake-up waits, even for an instant already past: that is what keeps
            // a due step from costing a CPU-bound spin, and it hands the runtime back
            // between wake-ups.
            ::tokio::time::sleep(pending.wait(idle_shift)).await;
            let mut performed = 0;
            for pipeline in &pending.pipelines {
                performed +=
                    super::driver::advance_pipeline(&driven, &callback, &region, pipeline).await;
            }
            idle_shift = match performed {
                0 => (idle_shift + 1).min(MAX_IDLE_SHIFT),
                _ => 0,
            };
            // A wake-up that performed work leaves the clock as fresh as a new one, so
            // only the ones that found nothing count against the cap.
            idle_wake_ups = match performed {
                0 => idle_wake_ups + 1,
                _ => 0,
            };
        }
        if capped {
            // An execution is still in flight and this clock is spent, so say so: from
            // here the steps it owes wait for the next call to this service, which is
            // the one thing a client watching only the artifact store never makes.
            ::tracing::warn!(
                "the execution clock stood down after {MAX_IDLE_WAKE_UPS} wake-ups that \
                 found nothing due while an execution was still in flight; the next call \
                 to this service materialises what it owes"
            );
            // Only THIS clock's claim, so a clock started since is left holding its own.
            release(&mut driven.lock(), generation);
        }
    });
    set_clock(&mut state.lock(), generation, handle);
}

/// What one wake-up found in flight: the pipelines whose executions are not finished,
/// and the earliest instant any of their steps is due at.
struct Pending {
    /// The pipelines to advance, each at most once.
    pipelines: BTreeSet<String>,
    /// The earliest stored due instant among their unfinished steps, if any step carries
    /// one. An instant already past is kept as it is: the wake-up still waits (see
    /// [`Self::wait`]), because the floor is what bounds the rate.
    next_due: Option<SystemTime>,
}

impl Pending {
    /// How long this wake-up waits before it advances: until the earliest due instant,
    /// never less than the idle floor and never more than [`MAX_TICK`].
    fn wait(&self, idle_shift: u32) -> Duration {
        let floor = (MIN_TICK * (1 << idle_shift)).min(MAX_TICK);
        self.next_due
            .and_then(|due| due.duration_since(SystemTime::now()).ok())
            .unwrap_or_default()
            .clamp(floor, MAX_TICK)
    }
}

/// What is still in flight — or `None`, having RELEASED the claim, when nothing is.
///
/// Observing "nothing is in flight" and standing down happen in ONE critical section,
/// which is what makes the hand-over race-free: a call that starts an execution holds
/// this same lock to store it BEFORE it tries to claim the clock, so either this call
/// sees that execution and the running clock keeps it, or the starter's own claim finds
/// the flag cleared and starts a clock of its own.
fn pending_or_release(
    store: &mut ComAmazonawsCodepipelineCodepipeline20150709State,
    generation: u64,
) -> Option<Pending> {
    let mut pending = Pending {
        pipelines: BTreeSet::new(),
        next_due: None,
    };
    for record in store.executions.values() {
        if is_terminal_status(&record.status) {
            continue;
        }
        pending.pipelines.insert(record.pipeline_name.clone());
        let due = record
            .stages
            .iter()
            .filter(|stage| !stage.is_terminal())
            .flat_map(|stage| {
                stage.entry_due.into_iter().chain(
                    stage
                        .actions
                        .iter()
                        .filter(|action| !action.is_terminal())
                        .filter_map(|action| action.due),
                )
            })
            .min();
        pending.next_due = pending.next_due.into_iter().chain(due).min();
    }
    if pending.pipelines.is_empty() {
        release(store, generation);
        return None;
    }
    Some(pending)
}

/// Claim the single clock for this store: the claim's GENERATION when the caller may
/// start one, `None` when one is really running.
///
/// The claim is taken over from a clock whose TASK HAS ALREADY ENDED while still holding
/// it. A clock releases its claim itself on the two ends it can see coming — nothing left
/// in flight, and the idle cap — and [`release`] drops the handle with the claim, so a
/// claim still held while the task is over belongs to a clock that ended on neither: one
/// wake-up's work panicked, or the task was cancelled. Without the take-over that claim
/// is held for as long as this store lives, and because the claim is what stops a second
/// clock from starting, no later call could ever start one again: every step an execution
/// owes would wait for a caller to materialise it, which is the one thing the clock
/// exists to avoid.
///
/// A granted claim takes the OLD handle out with it, so the window between this call and
/// the new handle being installed ([`set_clock`]) holds no handle at all: a call that
/// reaches this one inside that window sees a held claim and no ended task, and starts
/// nothing. One clock runs at a time.
fn claim(store: &mut ComAmazonawsCodepipelineCodepipeline20150709State) -> Option<u64> {
    let ended = store
        .clock
        .as_ref()
        .is_some_and(::tokio::task::JoinHandle::is_finished);
    if store.clock_running && !ended {
        return None;
    }
    if ended && store.clock_running {
        ::tracing::warn!(
            "the execution clock this store holds the claim for has ended without releasing \
             it; the claim is taken over so the steps it owes are still materialised"
        );
    }
    // The task this handle names has ended, or has released its claim and is ending, so
    // nothing is lost by dropping it; the abort is what makes that true in the second
    // case as well.
    if let Some(previous) = store.clock.take() {
        previous.abort();
    }
    store.clock_running = true;
    store.clock_generation = store.clock_generation.wrapping_add(1);
    Some(store.clock_generation)
}

/// Release the claim `generation` took, and the handle with it — and ONLY that one, so a
/// clock that ended on the idle cap cannot clear a claim a clock started since is
/// holding.
fn release(store: &mut ComAmazonawsCodepipelineCodepipeline20150709State, generation: u64) {
    if store.clock_generation == generation {
        store.clock_running = false;
        store.clock = None;
    }
}

/// Keep the running clock's task handle, aborting the one it replaces — and only while
/// `generation` is still the CURRENT claim, so a starter whose own task has already stood
/// down installs nothing but its own abort.
///
/// An abort leaves exactly what a request dropped mid-flight leaves: the step it was
/// performing stays CLAIMED on the action until `execution::CLAIM_TIMEOUT` passes, after
/// which the next clock or call performs it again. Nothing is reported as done for work
/// that did not finish, because the outcome is stored in one critical section after the
/// work returns.
fn set_clock(
    store: &mut ComAmazonawsCodepipelineCodepipeline20150709State,
    generation: u64,
    handle: ::tokio::task::JoinHandle<()>,
) {
    if store.clock_generation != generation {
        handle.abort();
        return;
    }
    if let Some(previous) = store.clock.replace(handle) {
        previous.abort();
    }
}
