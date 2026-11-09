// SPDX-License-Identifier: CC0-1.0

//! The INVOCATION half of EventBridge Scheduler: the clock that notices which
//! schedules the expressions have made due, and the hand-over that reaches each
//! one's target.
//!
//! EventBridge Scheduler is a data-plane service. A schedule in `State=ENABLED`
//! "invokes the target" at each occurrence its `ScheduleExpression` names
//! ([`super::occurrence`]), inside the window `StartDate`/`EndDate` bound, and at
//! each occurrence it acts as the `Target$RoleArn` — "the Amazon Resource Name
//! (ARN) of the IAM role that EventBridge Scheduler will use for this target when
//! the schedule is invoked" — and hands `Target$Input`, "the text, or well-formed
//! JSON, passed to the target", to the API that target's service is invoked by.
//! Nothing a client calls makes that happen: a client writes a schedule and then
//! watches the TARGET, so this clock is what makes a schedule observable at all.
//!
//! It is the same shape as the schedule clock EventBridge's own rules are fired by:
//! at most one task per store, holding a claim it records a tick against, started
//! by whichever request last touched a schedule, and nothing sleeps for a due
//! instant — the instant is stored and the clock performs what it has already
//! made due.
//!
//! ## Which schedule is on a clock
//!
//! One whose `State` is `ENABLED` — "specifies whether the schedule is enabled or
//! disabled", and a disabled schedule invokes nothing — in a group whose deletion
//! is not under way, and whose `ScheduleExpression` names occurrences this emulator
//! can compute. A schedule that stops satisfying any of those (deleted, disabled,
//! re-pointed, its group deleted) keeps no due instant, so re-enabling one starts
//! its series afresh rather than owing the invocations it missed.
//!
//! ## When this emulator performs an occurrence
//!
//! The occurrence's own instant is the schedule's — computed from its expression,
//! in its zone, and reported to the target as the scheduled time — but the WAIT
//! until it is compressed onto this emulator's clock
//! ([`EMULATED_WAIT_PER_STATED_MINUTE`]), the way every other wait this emulator
//! serves inside one caller's poll loop is. A schedule states its waits in minutes
//! and up (the shortest `rate` is one minute, an `at(...)` instant is written by a
//! client minutes ahead), and a wait of that length closes after every client
//! watching the target has given up, which would make the invocation the schedule
//! asked for unobservable. What is NOT emulator-chosen is WHICH instants the
//! expression names, their ORDER, how MANY of them there are, the window that
//! bounds them, or the scheduled time the target is told — all of those come from
//! the schedule itself.
//!
//! ## What reaches the target
//!
//! `Target$Input` with the schedule's context attributes substituted
//! ([`rendered_input`]), handed to the one API the target's service is invoked by:
//! `sqs:SendMessage` for a queue, `sns:Publish` for a topic,
//! `lambda:InvokeFunction` for a function and `states:StartExecution` for a state
//! machine. The call is made over the in-process callback, so what the target holds
//! afterwards is what that service's own handlers wrote.
//!
//! The retry policy, the dead-letter route a terminally failed invocation takes,
//! `ActionAfterCompletion` and the remaining target kinds (the ECS task, the
//! EventBridge bus, the SageMaker pipeline and the universal `aws-sdk` target) are
//! NOT performed here: an occurrence whose target kind this does not invoke is
//! recorded as not attempted, and nothing is reported as delivered that was not.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::Target;

use super::{
    occurrence::Occurrences, service_state, ComAmazonawsSchedulerAwschronosserviceState,
    ScheduleRecord, FALLBACK_REGION,
};

/// How much of this emulator's clock one minute of a schedule's own wait takes.
///
/// EMULATOR-CHOSEN, and the one place the compression lives. EventBridge Scheduler
/// states its cadences in minutes at the finest ("rate(value unit) […] minute |
/// minutes | hour | hours | day | days") and a one-time `at(...)` instant is
/// written minutes or hours ahead, while an emulated invocation is observed inside
/// a single caller's poll loop: a wait served at its stated length would close
/// after every client watching the target had given up, so the schedule's
/// invocation would be unobservable rather than late. The compression is
/// PROPORTIONAL and so monotonic — a schedule that waits longer is never invoked
/// sooner than one that waits less — and it changes only WHEN this emulator
/// performs an occurrence, never which instants the expression names nor the
/// scheduled time the target is told.
pub const EMULATED_WAIT_PER_STATED_MINUTE: Duration = Duration::from_secs(1);

/// The smallest wait this emulator serves before an occurrence, however short the
/// compressed wait would otherwise be.
///
/// Deliberately non-zero: a client that writes a schedule and then looks at the
/// target still observes the state before the first invocation, and a schedule
/// whose first occurrence is seconds away is not invoked inside the call that
/// created it.
pub const MINIMUM_EMULATED_WAIT: Duration = Duration::from_secs(1);

/// How often the clock wakes up to ask what is due.
///
/// Emulator-chosen: fine enough that an invocation is performed close to the
/// instant it fell due on this clock, and coarse enough that an idle emulator
/// spends nothing noticeable on it.
const CLOCK_TICK: Duration = Duration::from_millis(250);

/// How many ticks one clock may take before it stands down, so a task started by
/// one request cannot outlive the process it was started in by much: at
/// [`CLOCK_TICK`] a tick, two hours. A later call that writes a schedule starts a
/// fresh one ([`drive_invocation_clock`]).
const MAX_CLOCK_TICKS: usize = 28_800;

/// After how long with no tick recorded another caller may take the clock over.
///
/// The claim below is held by whichever clock last recorded a tick; a task that
/// ENDED without releasing it (a panic, a runtime shut down under it) would
/// otherwise keep every schedule from ever being invoked again, so the claim lapses
/// after this much silence and the next call that writes a schedule starts a clock
/// that takes it. It is longer than one invocation can take, so a clock busy
/// delivering is never taken over mid-invocation.
const CLAIM_LAPSES_AFTER: Duration = Duration::from_secs(120);

/// The `ScheduleState` that invokes nothing: "DISABLED […] specifies whether the
/// schedule is enabled or disabled".
const SCHEDULE_STATE_DISABLED: &str = "DISABLED";

/// The format the scheduled-time context attribute carries, as EventBridge
/// Scheduler publishes it (`2022-03-10T13:00:00Z`): the occurrence's own instant in
/// UTC, to the second.
const SCHEDULED_TIME_FORMAT: &str = "%Y-%m-%dT%H:%M:%SZ";

/// The context attributes a target's `Input` may carry, which EventBridge Scheduler
/// substitutes on every invocation ("schedule context attributes": the schedule's
/// ARN, the occurrence's scheduled time, the invocation's execution id and the
/// attempt this delivery is).
const SCHEDULE_ARN_ATTRIBUTE: &str = "<aws.scheduler.schedule-arn>";
/// See [`SCHEDULE_ARN_ATTRIBUTE`].
const SCHEDULED_TIME_ATTRIBUTE: &str = "<aws.scheduler.scheduled-time>";
/// See [`SCHEDULE_ARN_ATTRIBUTE`].
const EXECUTION_ID_ATTRIBUTE: &str = "<aws.scheduler.execution-id>";
/// See [`SCHEDULE_ARN_ATTRIBUTE`].
const ATTEMPT_NUMBER_ATTRIBUTE: &str = "<aws.scheduler.attempt-number>";

/// The attempt an invocation this module performs is: its first. A retry policy is
/// not applied here, so no delivery is ever a later attempt.
const FIRST_ATTEMPT: &str = "1";

/// What a target with NO `Input` of its own is handed.
///
/// `Target$Input` is optional and the service documents only that it "delivers a
/// default notification to the target" when a schedule states none, without
/// publishing that notification's content — so the empty JSON document is what this
/// emulator hands such a target, and it is CHOSEN rather than read off the service.
const DEFAULT_INPUT: &str = "{}";

/// The ARN namespaces of the target kinds this module invokes, each the
/// `arnNamespace` of the service whose API carries the invocation.
const SQS_ARN_NAMESPACE: &str = "sqs";
/// See [`SQS_ARN_NAMESPACE`].
const SNS_ARN_NAMESPACE: &str = "sns";
/// See [`SQS_ARN_NAMESPACE`].
const LAMBDA_ARN_NAMESPACE: &str = "lambda";
/// See [`SQS_ARN_NAMESPACE`].
const STATES_ARN_NAMESPACE: &str = "states";

/// What the clock has done and which clock is running it: the whole of this
/// module's state, held in the store so it survives the task that reads it.
#[derive(Debug, Default)]
pub struct Invocations {
    /// The next occurrence of each schedule on a clock, by the schedule's own ARN.
    armed: BTreeMap<String, Armed>,
    /// The generation of the clock currently driving these schedules, with the
    /// instant it last recorded a tick: see [`CLAIM_LAPSES_AFTER`].
    clock: Option<(u64, SystemTime)>,
    /// The generations handed out so far, so no two clocks of one run share one.
    generation: u64,
    /// The task the clock of that generation runs as, so it stays ABORTABLE: a
    /// clock whose claim is taken over is stopped rather than left running beside
    /// the one that took it.
    running: Option<::tokio::task::JoinHandle<()>>,
    /// How many invocations this store has performed, which is what makes each
    /// execution id its own ([`Self::next_execution_id`]).
    performed: u64,
}

/// One schedule's next occurrence, as the clock holds it.
#[derive(Debug, Clone)]
struct Armed {
    /// The definition the entry was computed from, so a schedule an
    /// `UpdateSchedule` re-pointed is re-armed rather than invoked on the series it
    /// no longer states ([`definition_of`]).
    definition: String,
    /// The occurrence's OWN instant, the one the expression names and the target is
    /// told: it is what the next occurrence is computed from, so the series keeps
    /// the schedule's cadence however the waits are served.
    nominal: SystemTime,
    /// When this emulator performs that occurrence, or `None` for a series with no
    /// occurrence left — a one-time schedule that has been invoked, or a recurring
    /// one past its `EndDate`.
    ///
    /// Such a schedule keeps its entry rather than losing it: the occurrence this
    /// emulator has already performed is still AHEAD on the schedule's own clock —
    /// that is what a compressed wait means — so an entry that was dropped would be
    /// read as a schedule the clock has not seen yet and the one occurrence would
    /// be performed over and over.
    due: Option<SystemTime>,
}

/// One schedule the clock may invoke: how it is addressed, the series it runs on,
/// and the window that series is bounded by.
#[derive(Debug, Clone)]
struct Schedulable {
    group: String,
    name: String,
    arn: String,
    definition: String,
    occurrences: Occurrences,
    /// "The date, after which the schedule can begin invoking its target"
    /// (`StartDate`), when the schedule states one.
    start: Option<SystemTime>,
    /// "The date, before which the schedule can invoke its target" (`EndDate`),
    /// when the schedule states one.
    end: Option<SystemTime>,
}

impl Schedulable {
    /// The first occurrence after `after` that falls inside this schedule's window,
    /// or `None` when the series has none left.
    ///
    /// The window is the schedule's: an occurrence before `StartDate` is not one it
    /// "can begin invoking its target" at, and one at or past `EndDate` is not one
    /// it "can invoke its target" before. `EndDate` bounds a recurring series only
    /// — "EventBridge Scheduler ignores StartDate and EndDate for one-time
    /// schedules" — so an `at(...)` instant is kept whatever those members state.
    fn next_after(&self, after: SystemTime) -> Option<SystemTime> {
        let one_time = self.occurrences.is_one_time();
        let from = match self.start {
            Some(start) if !one_time && start > after => start,
            _ => after,
        };
        let next = self.occurrences.next_after(from)?;
        match self.end {
            Some(end) if !one_time && next >= end => None,
            _ => Some(next),
        }
    }
}

/// One occurrence the clock has made due: which schedule, and the occurrence's own
/// instant.
#[derive(Debug, Clone)]
struct Firing {
    schedule: Schedulable,
    nominal: SystemTime,
}

impl Invocations {
    /// Claim the single clock for this store, answering the generation the caller's
    /// clock runs under — `None` when a clock is really running, which is what
    /// keeps one clock per store however many calls ask.
    fn claim(&mut self, now: SystemTime) -> Option<u64> {
        if let Some((_, ticked)) = self.clock {
            let lapsed = now
                .duration_since(ticked)
                .is_ok_and(|silent| silent >= CLAIM_LAPSES_AFTER);
            if !lapsed {
                return None;
            }
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.clock = Some((generation, now));
        Some(generation)
    }

    /// Record that the clock of `generation` is still running, answering whether it
    /// still holds the claim: one that has been taken over stands down rather than
    /// invoking beside the clock that took it.
    fn ticked(&mut self, generation: u64, now: SystemTime) -> bool {
        match self.clock {
            Some((held, _)) if held == generation => {
                self.clock = Some((generation, now));
                true
            }
            _ => false,
        }
    }

    /// Release the claim held by `generation`, leaving a claim taken over since
    /// alone.
    fn release(&mut self, generation: u64) {
        if matches!(self.clock, Some((held, _)) if held == generation) {
            self.clock = None;
            self.running = None;
        }
    }

    /// Hold the task the clock just started, stopping whichever task the claim it
    /// took was held by: at most one clock task is alive per store, and it can
    /// always be stopped.
    fn set_running(&mut self, running: ::tokio::task::JoinHandle<()>) {
        if let Some(previous) = self.running.replace(running) {
            previous.abort();
        }
    }

    /// The execution id of the next invocation this store performs.
    ///
    /// EventBridge Scheduler names every invocation ("the execution id" a target's
    /// `Input` may carry), and the form it publishes is a UUID, rendered here
    /// through this tree's one rendering of 16 opaque bytes as one. The bytes are
    /// this store's own invocation counter and its complement, so no two
    /// invocations of one run share an id.
    fn next_execution_id(&mut self) -> String {
        self.performed = self.performed.wrapping_add(1);
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&(!self.performed).to_be_bytes());
        bytes[8..].copy_from_slice(&self.performed.to_be_bytes());
        ::service_common::uuid_from_random_bytes(bytes)
    }

    /// Which schedules are DUE now, and the occurrence each one's clock moves to.
    ///
    /// Both halves happen here, under one lock: the invocation is performed outside
    /// it, so the due instant has to move BEFORE the delivery rather than after it —
    /// otherwise a slow delivery would make the same occurrence due twice.
    fn take_due(&mut self, schedulable: &[Schedulable], now: SystemTime) -> Vec<Firing> {
        let mut firing = Vec::new();
        let mut known: BTreeMap<String, Armed> = BTreeMap::new();
        for schedule in schedulable {
            let held = self
                .armed
                .get(&schedule.arn)
                .filter(|armed| armed.definition == schedule.definition);
            let Some(armed) = held else {
                // A schedule this clock has not seen yet, or one whose definition a
                // write has changed: its next occurrence is computed from now, so
                // the series a client just asked for starts where the client asked
                // for it.
                known.insert(
                    schedule.arn.clone(),
                    match schedule.next_after(now) {
                        Some(nominal) => armed_for(schedule, nominal, now, now),
                        None => exhausted(schedule, now),
                    },
                );
                continue;
            };
            let Some(due) = armed.due else {
                // A series with nothing left is remembered, and asked no more.
                known.insert(schedule.arn.clone(), armed.clone());
                continue;
            };
            if due > now {
                known.insert(schedule.arn.clone(), armed.clone());
                continue;
            }
            let nominal = armed.nominal;
            // The occurrence AFTER the one being performed is computed from the
            // occurrence's own instant, so the series keeps the cadence the
            // expression states rather than drifting by however long a delivery
            // took.
            known.insert(
                schedule.arn.clone(),
                match schedule.next_after(nominal) {
                    Some(next) => armed_for(schedule, next, nominal, now),
                    None => Armed {
                        definition: schedule.definition.clone(),
                        nominal,
                        due: None,
                    },
                },
            );
            firing.push(Firing {
                schedule: schedule.clone(),
                nominal,
            });
        }
        self.armed = known;
        firing
    }
}

/// The clock entry for a schedule whose series names no occurrence at all — a
/// one-time instant already behind the clock, or a window that holds none.
fn exhausted(schedule: &Schedulable, now: SystemTime) -> Armed {
    Armed {
        definition: schedule.definition.clone(),
        nominal: now,
        due: None,
    }
}

/// The clock entry for `nominal`: the occurrence's own instant, and when this
/// emulator performs it — `now` plus the compressed form of the wait the schedule
/// states between `from` and that occurrence.
fn armed_for(
    schedule: &Schedulable,
    nominal: SystemTime,
    from: SystemTime,
    now: SystemTime,
) -> Armed {
    let stated = nominal.duration_since(from).unwrap_or_default();
    Armed {
        definition: schedule.definition.clone(),
        nominal,
        due: Some(after(now, emulated_wait(stated))),
    }
}

/// The wait this emulator serves for a schedule that states a wait of `stated`.
///
/// Proportional to the stated wait ([`EMULATED_WAIT_PER_STATED_MINUTE`]) and never
/// shorter than [`MINIMUM_EMULATED_WAIT`], so the order of two waits is the order
/// of the two it serves and no occurrence is performed the instant it is armed.
#[must_use]
pub fn emulated_wait(stated: Duration) -> Duration {
    /// Seconds in the minute the scale is stated per.
    const SECONDS_PER_MINUTE: u128 = 60;

    let scaled = stated
        .as_millis()
        .saturating_mul(EMULATED_WAIT_PER_STATED_MINUTE.as_millis())
        / SECONDS_PER_MINUTE.saturating_mul(1_000);
    let scaled = u64::try_from(scaled).unwrap_or(u64::MAX);
    Duration::from_millis(scaled).max(MINIMUM_EMULATED_WAIT)
}

/// `at` plus `span`, saturating rather than panicking on a clock near the end of
/// its range.
fn after(at: SystemTime, span: Duration) -> SystemTime {
    at.checked_add(span).unwrap_or(at)
}

/// The DEFINITION a schedule's series is computed from: every member of it that
/// decides which instants it names, which target they reach and under which role.
///
/// A write that changes any of them re-arms the schedule, so a client that
/// re-points or re-expresses one is not served the series the schedule no longer
/// states.
fn definition_of(record: &ScheduleRecord) -> String {
    format!(
        "{}|{}|{:?}|{:?}|{}|{}|{}|{:?}",
        record.schedule_expression,
        record
            .schedule_expression_timezone
            .as_deref()
            .unwrap_or_default(),
        record.start_date.map(|at| (at.secs(), at.subsec_nanos())),
        record.end_date.map(|at| (at.secs(), at.subsec_nanos())),
        record.state,
        record.target.arn,
        record.target.role_arn,
        record.target.input,
    )
}

/// Every schedule of this store that is on a clock, in the order the store holds
/// them.
fn schedulable(store: &ComAmazonawsSchedulerAwschronosserviceState) -> Vec<Schedulable> {
    store
        .schedules
        .values()
        .filter(|record| !store.group_is_deleting(&record.group_name))
        .filter_map(schedulable_record)
        .collect()
}

/// The schedulable form of one record, or `None` when it invokes nothing: a
/// `DISABLED` schedule, or an expression this emulator computes no occurrence from.
fn schedulable_record(record: &ScheduleRecord) -> Option<Schedulable> {
    if record.state == SCHEDULE_STATE_DISABLED {
        return None;
    }
    let occurrences = Occurrences::parse(
        &record.schedule_expression,
        record.schedule_expression_timezone.as_deref(),
    )?;
    Some(Schedulable {
        group: record.group_name.clone(),
        name: record.name.clone(),
        arn: record.arn.clone(),
        definition: definition_of(record),
        occurrences,
        start: record.start_date.map(wire_instant),
        end: record.end_date.map(wire_instant),
    })
}

/// One stored wire timestamp as a clock reading.
fn wire_instant(at: ::aws_smithy_types::DateTime) -> SystemTime {
    SystemTime::try_from(at).unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Start the one in-process task that invokes this store's schedules as the clock
/// makes them due, if one is not already running.
///
/// Called by every operation that WRITES a schedule, so a schedule is on a clock
/// from the moment a client asks for it, whether the client called `CreateSchedule`
/// itself or a stack deployed an `AWS::Scheduler::Schedule` that did. With no async
/// runtime to spawn onto there is no clock — which is the case in a unit test that
/// drives a handler directly, where nothing is watching a target either.
///
/// `callback` is the in-process emulator every invocation is made through, cloned
/// from the request that started the clock: the calls this clock makes reach the
/// same handlers a client's own calls reach.
pub fn drive_invocation_clock<Eng: Send + Sync + 'static>(
    engine: &::std::sync::Arc<Eng>,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let now = SystemTime::now();
    let Some(generation) = state.lock().invocations.claim(now) else {
        return;
    };
    let callback = callback.clone();
    let claimed = ::engine::ServiceState::clone(&state);
    let engine = ::std::sync::Arc::clone(engine);
    let running = runtime.spawn(async move {
        for _ in 0..MAX_CLOCK_TICKS {
            ::tokio::time::sleep(CLOCK_TICK).await;
            let Some(state) = service_state(&*engine) else {
                return;
            };
            let now = SystemTime::now();
            let firing = {
                let mut store = state.lock();
                if !store.invocations.ticked(generation, now) {
                    return;
                }
                // Every group removal the clock already owed is materialised
                // first, so a schedule whose group has gone is not invoked.
                store.settle(now);
                let schedulable = schedulable(&store);
                store.invocations.take_due(&schedulable, now)
            };
            for occurrence in firing {
                invoke(&state, &callback, &occurrence).await;
            }
        }
        state.lock().invocations.release(generation);
    });
    claimed.lock().invocations.set_running(running);
}

/// Perform ONE occurrence: act as the schedule's execution role and hand its
/// target the rendered input.
///
/// The record is read HERE, with the store locked only for as long as reading it
/// takes: an invocation reaches another service's handler, which takes that
/// service's own lock.
async fn invoke(
    state: &::engine::ServiceState<ComAmazonawsSchedulerAwschronosserviceState>,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    occurrence: &Firing,
) {
    let schedule = &occurrence.schedule;
    let (target, execution_id) = {
        let mut store = state.lock();
        let Some(record) = store
            .schedules
            .get(&(schedule.group.clone(), schedule.name.clone()))
            .filter(|record| record.state != SCHEDULE_STATE_DISABLED)
            .cloned()
        else {
            return;
        };
        let execution_id = store.invocations.next_execution_id();
        (record.target, execution_id)
    };
    let region = ::service_common::arn::region_of(&schedule.arn, "scheduler")
        .unwrap_or(FALLBACK_REGION)
        .to_string();
    // The role is assumed BEFORE anything is invoked: "EventBridge Scheduler will
    // use [this role] for this target when the schedule is invoked", so a role this
    // account no longer holds, or one whose trust policy no longer admits the
    // scheduler, invokes nothing.
    if let Some(refusal) =
        super::execution_role_violation(callback, &region, &target.role_arn).await
    {
        ::tracing::warn!(
            schedule = %schedule.arn,
            role = %target.role_arn,
            %refusal,
            "an EventBridge Scheduler invocation was withheld: its execution role could not \
             be assumed"
        );
        return;
    }
    let body = rendered_input(&target.input, &schedule.arn, occurrence.nominal, &execution_id);
    deliver(callback, &region, &schedule.arn, &target, &body).await;
}

/// Hand `body` to the target `target.arn` names, through the one API that target's
/// service is invoked by.
///
/// The target is reached in ITS OWN Region — the one its ARN names — falling back
/// to the schedule's when the ARN carries no Region label.
async fn deliver(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    schedule_arn: &str,
    target: &Target,
    body: &str,
) {
    let arn = target.arn.as_str();
    let Some(parsed) = ::service_common::arn::parse(arn) else {
        ::tracing::warn!(
            schedule = %schedule_arn,
            target = %arn,
            "an EventBridge Scheduler invocation was not attempted: the target ARN names no \
             resource"
        );
        return;
    };
    let region = ::service_common::arn::region_of(arn, parsed.vendor).unwrap_or(region);
    let failure = match parsed.vendor {
        SQS_ARN_NAMESPACE => {
            let Some(queue_url) = queue_url(region, parsed.account, parsed.resource) else {
                ::tracing::warn!(
                    schedule = %schedule_arn,
                    target = %arn,
                    "an EventBridge Scheduler invocation was not attempted: the target ARN \
                     names no queue"
                );
                return;
            };
            let mut request = sqs_client(callback, region)
                .send_message()
                .queue_url(queue_url)
                .message_body(body);
            // "The FIFO message group ID to use as the target" (`SqsParameters`):
            // such a queue takes no message without one, so the group the target
            // names travels with the invocation.
            if let Some(group) = target
                .sqs_parameters
                .as_ref()
                .and_then(|parameters| parameters.message_group_id.as_deref())
            {
                request = request.message_group_id(group);
            }
            request.send().await.err().map(render_error)
        }
        SNS_ARN_NAMESPACE => sns_client(callback, region)
            .publish()
            .topic_arn(arn)
            .message(body)
            .send()
            .await
            .err()
            .map(render_error),
        LAMBDA_ARN_NAMESPACE => lambda_client(callback, region)
            .invoke()
            .function_name(arn)
            // A schedule does not wait for its function target's answer.
            .invocation_type(::aws_sdk_lambda::types::InvocationType::Event)
            .payload(::aws_sdk_lambda::primitives::Blob::new(body.as_bytes()))
            .send()
            .await
            .err()
            .map(render_error),
        STATES_ARN_NAMESPACE => sfn_client(callback, region)
            .start_execution()
            .state_machine_arn(arn)
            .input(body)
            .send()
            .await
            .err()
            .map(render_error),
        _ => Some(format!(
            "this emulator does not invoke a target of {}",
            parsed.vendor
        )),
    };
    if let Some(failure) = failure {
        ::tracing::warn!(
            schedule = %schedule_arn,
            target = %arn,
            %failure,
            "an EventBridge Scheduler invocation did not reach its target"
        );
    }
}

/// One SDK failure as the sentence a warning carries.
fn render_error<E: ::std::fmt::Debug>(error: E) -> String {
    format!("{error:?}")
}

/// The `QueueUrl` Amazon SQS addresses the queue of `name` in `account` by, or
/// `None` when the ARN's fields name no queue.
///
/// A queue's ARN names the queue DIRECTLY
/// (`arn:<partition>:sqs:<region>:<account>:<QueueName>`), and the url every SQS
/// operation takes is `<endpoint>/<account>/<QueueName>`. A resource field carrying
/// a type separator addresses no queue, and an account field that is not an account
/// id names none either — so neither reaches the url, which is what keeps a
/// caller-written ARN from reshaping the address this invocation is made to. The
/// Region in the host is the one the shared ARN reader already held to the Region
/// grammar.
fn queue_url(region: &str, account: &str, name: &str) -> Option<String> {
    /// How many digits an account id carries.
    const ACCOUNT_ID_DIGITS: usize = 12;

    let account_names_an_account =
        account.len() == ACCOUNT_ID_DIGITS && account.bytes().all(|byte| byte.is_ascii_digit());
    if !account_names_an_account || name.contains('/') || name.contains(':') {
        return None;
    }
    Some(format!("https://sqs.{region}.amazonaws.com/{account}/{name}"))
}

/// Build one client over the IN-PROCESS callback into this same emulator, against
/// the target service's endpoint in `region`.
///
/// The transport is in-process, so each client disables what a network client needs
/// and this one cannot have: retries, timeouts, stalled-stream protection and
/// identity caching each need a sleep implementation the callback transport is not
/// given, and there is nothing to retry behind it anyway.
macro_rules! callback_client {
    ($name:ident, $sdk:ident, $endpoint:literal) => {
        fn $name(
            callback: &::smithy_server_runtime::EmbeddedEmulator,
            region: &str,
        ) -> ::$sdk::Client {
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
            ::$sdk::Client::from_conf(config)
        }
    };
}

callback_client!(sqs_client, aws_sdk_sqs, "sqs");
callback_client!(sns_client, aws_sdk_sns, "sns");
callback_client!(lambda_client, aws_sdk_lambda, "lambda");
// Step Functions publishes its endpoint and signs under `states`.
callback_client!(sfn_client, aws_sdk_sfn, "states");

/// The text one occurrence hands its target: the target's own `Input` with the
/// schedule's context attributes substituted, or [`DEFAULT_INPUT`] for a target that
/// states none.
///
/// "The text, or well-formed JSON, passed to the target" travels VERBATIM —
/// nothing of the schedule is folded into it — except for the context attributes
/// EventBridge Scheduler substitutes, which are the schedule's ARN, the
/// occurrence's own scheduled time, this invocation's execution id and the attempt
/// it is.
#[must_use]
pub fn rendered_input(
    input: &Option<String>,
    schedule_arn: &str,
    scheduled_time: SystemTime,
    execution_id: &str,
) -> String {
    let Some(input) = input.as_deref().filter(|input| !input.is_empty()) else {
        return DEFAULT_INPUT.to_string();
    };
    input
        .replace(SCHEDULE_ARN_ATTRIBUTE, schedule_arn)
        .replace(SCHEDULED_TIME_ATTRIBUTE, &scheduled_time_text(scheduled_time))
        .replace(EXECUTION_ID_ATTRIBUTE, execution_id)
        .replace(ATTEMPT_NUMBER_ATTRIBUTE, FIRST_ATTEMPT)
}

/// One occurrence's instant as the scheduled-time attribute carries it.
#[must_use]
pub fn scheduled_time_text(at: SystemTime) -> String {
    ::chrono::DateTime::<::chrono::Utc>::from(at)
        .format(SCHEDULED_TIME_FORMAT)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clock reading `seconds` after the epoch.
    fn epoch(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    /// The wait this emulator serves is proportional to the one the schedule
    /// states, never shorter than the floor, and monotonic in it.
    #[test]
    fn the_served_wait_is_proportional_to_the_stated_one() {
        // (case, the wait the schedule states in seconds, the wait served in millis)
        let rows: [(&str, u64, u64); 7] = [
            ("no wait at all", 0, 1_000),
            ("under a minute", 30, 1_000),
            ("one minute, the shortest rate", 60, 1_000),
            ("four minutes, a one-time schedule written ahead", 240, 4_000),
            ("fifteen minutes", 900, 15_000),
            ("one hour", 3_600, 60_000),
            ("one day", 86_400, 1_440_000),
        ];
        let mut previous = Duration::ZERO;
        for (case, stated, expected) in rows {
            let served = emulated_wait(Duration::from_secs(stated));
            assert_eq!(
                served,
                Duration::from_millis(expected),
                "case {case:?}: expected {expected}ms, got {served:?}"
            );
            assert!(
                served >= previous,
                "case {case:?}: a longer stated wait must never be served sooner, got \
                 {served:?} after {previous:?}"
            );
            previous = served;
        }
    }

    /// The substituted input is the target's own text with the schedule's context
    /// attributes replaced, and a target stating no input is handed the default.
    #[test]
    fn an_input_carries_the_schedules_context_attributes() {
        let arn = "arn:aws:scheduler:eu-west-2:000000000000:schedule/default/nightly";
        let at = epoch(1_893_456_000);
        // (case, the target's Input, what the target is handed)
        let rows: [(&str, Option<&str>, &str); 6] = [
            ("no input at all", None, "{}"),
            ("an empty input", Some(""), "{}"),
            ("text carrying no attribute", Some("a marker"), "a marker"),
            (
                "the scheduled time",
                Some(r#"{"at":"<aws.scheduler.scheduled-time>"}"#),
                r#"{"at":"2030-01-01T00:00:00Z"}"#,
            ),
            (
                "the execution id and the attempt",
                Some(r#"{"id":"<aws.scheduler.execution-id>","try":<aws.scheduler.attempt-number>}"#),
                r#"{"id":"EXEC","try":1}"#,
            ),
            (
                "the schedule's own ARN",
                Some("<aws.scheduler.schedule-arn>"),
                arn,
            ),
        ];
        for (case, input, expected) in rows {
            let rendered = rendered_input(&input.map(ToString::to_string), arn, at, "EXEC");
            assert_eq!(
                rendered, expected,
                "case {case:?}: expected {expected:?}, got {rendered:?}"
            );
        }
    }

    /// A queue url is built only from fields that really name a queue of an
    /// account: the rows differ along the resource field and the account field.
    #[test]
    fn a_queue_url_is_built_only_from_fields_that_name_a_queue() {
        // (case, account, resource, the url the pair addresses)
        let rows: [(&str, &str, &str, Option<&str>); 6] = [
            (
                "a queue name",
                "000000000000",
                "orders",
                Some("https://sqs.eu-west-2.amazonaws.com/000000000000/orders"),
            ),
            (
                "a FIFO queue name",
                "000000000000",
                "orders.fifo",
                Some("https://sqs.eu-west-2.amazonaws.com/000000000000/orders.fifo"),
            ),
            ("a resource carrying a path", "000000000000", "../other", None),
            ("a resource carrying a type", "000000000000", "queue:orders", None),
            ("an account that is no account id", "my-account", "orders", None),
            ("an account of the wrong length", "0000", "orders", None),
        ];
        for (case, account, resource, expected) in rows {
            let built = queue_url("eu-west-2", account, resource);
            assert_eq!(
                built.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {built:?}"
            );
        }
    }

    /// Every execution id a store hands out is its own.
    #[test]
    fn each_invocation_is_named_once() {
        let mut invocations = Invocations::default();
        let mut seen = ::std::collections::BTreeSet::new();
        for index in 0..64 {
            let id = invocations.next_execution_id();
            assert!(
                seen.insert(id.clone()),
                "case invocation {index}: the id {id:?} was handed out twice"
            );
        }
    }
}
