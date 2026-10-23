// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-arcregionswitch-arcregionswitch service.
//!
//! Everything a handler persists about a Region switch plan and its executions lives
//! here, behind the engine lock, together with the projections that turn a stored
//! record into the modelled shapes (`Plan`, `AbbreviatedPlan`) the API reports. Handlers
//! never build those shapes from the request: they build them from the record.
//!
//! # Every store here is bounded
//!
//! This process is a developer's emulator that authenticates nobody, so a caller that
//! can reach it must not be able to grow its memory without limit. Each of the three
//! maps below has an explicit ceiling and a stated release path, and every insert goes
//! through the method that enforces it:
//!
//! | store | ceiling | release |
//! |---|---|---|
//! | [`ComAmazonawsArcregionswitchArcregionswitchState::plans`] | [`MAX_PLANS`] | `DeletePlan`, or the request is refused |
//! | [`ComAmazonawsArcregionswitchArcregionswitchState::executions`] | [`MAX_EXECUTIONS_PER_PLAN`] per plan | the oldest terminal execution is evicted; deleting the plan drops them all |
//! | [`ComAmazonawsArcregionswitchArcregionswitchState::client_tokens`] | [`MAX_CLIENT_TOKENS`] | the oldest record is evicted (FIFO) |
//!
//! Per-request cost is independent of how much is stored, too: [`settle`] walks only the
//! transitions whose deadline has passed, through the
//! [`ComAmazonawsArcregionswitchArcregionswitchState::due_transitions`] index, never the
//! whole execution map.
//!
//! [`settle`]: ComAmazonawsArcregionswitchArcregionswitchState::settle

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    AbbreviatedPlan, ExecutionBlockConfiguration, ExecutionEvent, Plan, Route53HealthCheck, Step,
    StepState,
};

/// How long after the call that starts one the clock owes an execution its NEXT
/// transition — the first step after `StartPlanExecution`, and the step after the one
/// `ApprovePlanExecutionStep` just released.
///
/// AWS documents no duration for it, so this is the emulator's own choice, inside the
/// project's 250-750 ms band for a transition the clock owes: a caller that reads the
/// execution straight after starting it still observes `pending`, and its state errors.
pub const EXECUTION_START_SPAN: Duration = Duration::from_millis(500);

/// The `ExecutionState` an execution is created in, before the clock has reached its
/// first step, from the model's `ExecutionState` enum
/// (`smithy_query(query="shape", shape="ExecutionState", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
pub const PENDING: &str = "pending";

/// The `ExecutionState` of an execution that is between steps: a step has just been
/// decided and the clock has not yet reached the next one.
pub const IN_PROGRESS: &str = "inProgress";

/// The `ExecutionState` of an execution the clock has stopped at a manual-approval step.
///
/// It is NOT terminal: the execution still holds the plan, the step itself reports
/// [`STEP_PENDING_APPROVAL`], and `ApprovePlanExecutionStep` is what moves it on.
pub const PENDING_MANUAL_APPROVAL: &str = "pendingManualApproval";

/// The `ExecutionState` of an execution every step of which reached
/// [`STEP_COMPLETED`] — including a workflow that configures no execution block at all,
/// which has nothing to perform.
pub const COMPLETED: &str = "completed";

/// The `ExecutionState` of an execution stopped by a declined approval.
pub const CANCELED: &str = "canceled";

/// The `ExecutionState` of an execution whose manual-approval step was neither approved
/// nor declined inside the window its own execution block configures.
pub const PLAN_EXECUTION_TIMED_OUT: &str = "planExecutionTimedOut";

/// The state an execution reaches when the clock gets to a step this emulator cannot
/// perform, from the model's `ExecutionState` enum.
///
/// It is the service's modelled FAILURE state, and it is the honest one: most of a
/// Region switch workflow's execution blocks drive OTHER services' resources (Lambda,
/// ECS, Auto Scaling, RDS, Route 53, ARC routing controls), which this emulator does not
/// run, so nothing performed that step. Reporting `completed` would claim an outcome
/// nothing produced, and staying `inProgress` for ever would report a run that is not
/// running AND never release the plan. Failing with [`UNSUPPORTED_EXECUTION_REASON`]
/// recorded on the execution's events says what happened and frees the plan.
///
/// A manual-approval block is the exception: it drives nothing outside this service, so
/// the clock really does reach it (see [`PENDING_MANUAL_APPROVAL`]).
pub const FAILED: &str = "failed";

/// Why a plan execution fails in this emulator, recorded on the `executionFailed` event
/// `ListPlanExecutionEvents` reports, so a caller is told rather than left guessing.
pub const UNSUPPORTED_EXECUTION_REASON: &str =
    "This emulator does not run a Region switch workflow's execution blocks, which drive \
     other services' resources, so the step this plan execution reached was not performed.";

/// Why an execution waiting for a manual approval ends in [`PLAN_EXECUTION_TIMED_OUT`],
/// recorded the same way.
pub const APPROVAL_TIMEOUT_REASON: &str =
    "The manual-approval step was neither approved nor declined inside the approval \
     window its execution block configures, so the plan execution timed out.";

/// The `StepStatus` of a step the clock has not reached, from the model's `StepStatus`
/// enum
/// (`smithy_query(query="shape", shape="StepStatus", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
pub const STEP_NOT_STARTED: &str = "notStarted";

/// The `StepStatus` of a manual-approval step the clock has reached and that is waiting
/// for `ApprovePlanExecutionStep`.
pub const STEP_PENDING_APPROVAL: &str = "pendingApproval";

/// The `StepStatus` of a step that is done: a manual approval that was approved.
pub const STEP_COMPLETED: &str = "completed";

/// The `StepStatus` of a step this emulator could not perform.
pub const STEP_FAILED: &str = "failed";

/// The `StepStatus` of a step that never ran because it was declined, or because its
/// approval window closed first.
pub const STEP_CANCELED: &str = "canceled";

/// The `ExecutionEventType` recorded when an execution is created.
pub const EVENT_EXECUTION_PENDING: &str = "executionPending";

/// The `ExecutionEventType` recorded when an execution reaches [`FAILED`] or
/// [`PLAN_EXECUTION_TIMED_OUT`]; the event's `error` says which it was and why. The
/// model's `ExecutionEventType` enum declares no timed-out member, so this is the
/// nearest it has.
pub const EVENT_EXECUTION_FAILED: &str = "executionFailed";

/// The `ExecutionEventType` recorded when the clock stops at a manual-approval step.
pub const EVENT_STEP_PENDING_APPROVAL: &str = "stepPendingApproval";

/// The `ExecutionEventType` recorded when a manual-approval step is approved.
pub const EVENT_STEP_SUCCEEDED: &str = "stepSucceeded";

/// The `ExecutionEventType` recorded when an execution reaches [`COMPLETED`].
pub const EVENT_EXECUTION_SUCCEEDED: &str = "executionSucceeded";

/// The `ExecutionEventType` recorded when a declined approval stops an execution.
pub const EVENT_EXECUTION_CANCELED: &str = "executionCanceled";

/// The approval window this emulator falls back to, in minutes.
///
/// It is the model's own default for the member that carries it
/// (`ExecutionApprovalConfiguration$timeoutMinutes`, "The timeout value specified for
/// the configuration", `@range(min: 1)`, `@default: 60`), so a stored block that somehow
/// carries no usable value waits exactly as long as one that spells the default out.
pub const DEFAULT_APPROVAL_TIMEOUT_MINUTES: u64 = 60;

/// The Region a request that carries no SigV4 credential scope is attributed to.
/// Matches the Region generated emulators default to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `Route53HealthCheckStatus` this emulator reports for a health check it lists,
/// from the model's `Route53HealthCheckStatus` enum
/// (`smithy_query(query="shape", shape="Route53HealthCheckStatus", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
///
/// Nothing in this emulator probes a Route 53 record's endpoint, so no health state has
/// been observed for it: `unknown` is the modelled value for exactly that. Reporting
/// `healthy` or `unhealthy` would claim an observation nothing made.
pub const HEALTH_CHECK_STATUS_UNKNOWN: &str = "unknown";

/// The most plans this emulator stores. Not an AWS quota — AWS publishes its own, which
/// this does not claim to reproduce — but the ceiling that keeps an unauthenticated
/// caller from growing the store without limit. `CreatePlan` refuses past it, and
/// `DeletePlan` is the release path.
pub const MAX_PLANS: usize = 500;

/// The most executions this emulator keeps per plan. Reaching it evicts the oldest
/// execution that has reached a state it never leaves; if none has, `StartPlanExecution`
/// is refused. (One plan can hold only one non-terminal execution at a time, so this
/// ceiling is what bounds the history that accumulates behind it.)
pub const MAX_EXECUTIONS_PER_PLAN: usize = 50;

/// The most `clientToken` records this emulator keeps. Reaching it evicts the oldest
/// recorded token, so the idempotency cache cannot accumulate without bound; a caller
/// whose token was evicted is answered as a fresh request rather than a replay.
pub const MAX_CLIENT_TOKENS: usize = 2_048;

/// The `ExecutionState` values an execution never leaves, from the model's
/// `ExecutionState` enum
/// (`smithy_query(query="shape", shape="ExecutionState", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
/// Every other value — `pending`, `inProgress`, `pausedByFailedStep`,
/// `pausedByOperator`, `pendingManualApproval` — is a state the execution can still
/// move out of, so it still holds the plan.
pub const TERMINAL_EXECUTION_STATES: &[&str] = &[
    CANCELED,
    COMPLETED,
    "completedMonitoringApplicationHealth",
    "completedWithExceptions",
    FAILED,
    PLAN_EXECUTION_TIMED_OUT,
];

/// The two Regions of a plan name one switch between them, so they have to be two
/// DIFFERENT Regions: `regions` is `@length(min: 2, max: 2)` (the boundary validator
/// refuses any other count before a handler runs) and an execution shifts traffic from
/// one of them to the other, which the same Region twice describes no such shift of.
///
/// `Some(message)` is the refusal, naming the member and the value, for a handler that
/// takes a plan's Region pair — today `CreatePlan`, and `UpdatePlan` once it is written.
pub fn region_pair_violation(regions: &[String]) -> Option<String> {
    let mut seen: BTreeSet<&String> = BTreeSet::new();
    let duplicate = regions.iter().find(|region| !seen.insert(region))?;
    Some(format!(
        "regions must name two different Amazon Web Services Regions, but {duplicate} is \
         named twice"
    ))
}

/// The `ResourceNotFoundException` message for a plan ARN this emulator holds no plan
/// under, built from the ARN the REQUEST carried.
///
/// One function so every operation that looks a plan up by ARN — `GetPlan`, `DeletePlan`,
/// `StartPlanExecution`, `ListRoute53HealthChecks`, `ListRoute53HealthChecksInRegion` —
/// renders that refusal identically.
pub fn plan_not_found_message(arn: &str) -> String {
    format!("Plan not found for ARN: {arn}")
}

/// The page size a paginated operation of this service serves.
///
/// Both `ListPlans` and `GetPlanExecution`/`ListPlanExecutionEvents` constrain
/// `maxResults` to `@range(1..100)`; an absent one asks for as many as that range allows.
/// One helper so the rule is written once.
pub fn page_size(requested: Option<i32>) -> usize {
    const MAX_PAGE_SIZE: i32 = 100;
    let clamped = requested.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
    usize::try_from(clamped).unwrap_or(1)
}

/// A Region switch plan as this emulator persists it: the modelled resource plus the
/// bookkeeping that is not part of the `Plan` shape.
#[derive(Debug, Clone)]
pub struct StoredPlan {
    /// The plan as the API reports it. `version` is NOT kept here — it is derived from
    /// [`StoredPlan::revision`], so there is one source of truth for it.
    pub plan: Plan,
    /// How many times the plan has been written. Rendered as the plan's `version`.
    pub revision: u64,
    /// The tags supplied for the plan, kept for `ListTagsForResource`/`TagResource`.
    pub tags: BTreeMap<String, String>,
    /// The ids of the executions started against this plan, oldest first. Bounded by
    /// [`MAX_EXECUTIONS_PER_PLAN`].
    pub executions: Vec<String>,
    /// The execution currently holding the plan, while it is in a non-terminal state.
    pub active_execution: Option<String>,
}

impl StoredPlan {
    /// The plan's `version`, derived from how many times it has been written.
    pub fn version(&self) -> String {
        self.revision.to_string()
    }

    /// The `Plan` shape for this record.
    pub fn as_plan(&self) -> Plan {
        let mut plan = self.plan.clone();
        plan.version = Some(self.version());
        plan
    }

    /// The `AbbreviatedPlan` projection `ListPlans` reports.
    pub fn as_abbreviated_plan(&self) -> AbbreviatedPlan {
        AbbreviatedPlan {
            arn: self.plan.arn.clone(),
            owner: self.plan.owner.clone(),
            name: self.plan.name.clone(),
            regions: self.plan.regions.clone(),
            recovery_approach: self.plan.recovery_approach.clone(),
            primary_region: self.plan.primary_region.clone(),
            version: Some(self.version()),
            updated_at: self.plan.updated_at,
            description: self.plan.description.clone(),
            execution_role: Some(self.plan.execution_role.clone()),
            active_plan_execution: self.active_execution.clone(),
            recovery_time_objective_minutes: self.plan.recovery_time_objective_minutes,
        }
    }

    /// The plan's other Region: a plan carries exactly two (`RegionList`, len 2..2), so
    /// the Region that is not `region` is unique when `region` is one of them.
    pub fn counterpart_region(&self, region: &str) -> Option<String> {
        self.plan.regions.iter().find(|r| *r != region).cloned()
    }

    /// The Route 53 health checks this plan configures, as
    /// `ListRoute53HealthChecks` reports them: one per (hosted zone, record name,
    /// Region) triple any `Route53HealthCheck` execution block of its workflows names,
    /// deduplicated and in ascending order of that triple so paging over them is stable.
    ///
    /// The triples come from the stored `Route53HealthCheckConfiguration`s: the block's
    /// `hostedZoneId` and `recordName`, with the `region` of each of its `recordSets`.
    /// `region` is a required member of `Route53HealthCheck`, and a record set's
    /// `region` is the only Region the stored configuration names, so a block whose
    /// record sets name none contributes nothing — this emulator does not read the
    /// hosted zone from Route 53 to discover the record sets a configuration left out.
    ///
    /// `healthCheckId` is absent and `status` is [`HEALTH_CHECK_STATUS_UNKNOWN`]: no
    /// Route 53 health check was created for the record, and nothing probed it.
    pub fn route53_health_checks(&self) -> Vec<Route53HealthCheck> {
        let mut found: BTreeSet<(String, String, String)> = BTreeSet::new();
        for workflow in &self.plan.workflows {
            collect_route53_health_checks(
                workflow.steps.as_deref().unwrap_or_default(),
                &mut found,
            );
        }
        found
            .into_iter()
            .map(
                |(hosted_zone_id, record_name, region)| Route53HealthCheck {
                    hosted_zone_id,
                    record_name,
                    health_check_id: None,
                    status: Some(HEALTH_CHECK_STATUS_UNKNOWN.to_string()),
                    region,
                },
            )
            .collect()
    }
}

/// Collect the `(hostedZoneId, recordName, region)` triples the `Route53HealthCheck`
/// blocks reachable from `steps` name, descending into the steps a `Parallel` block
/// nests.
///
/// The walk keeps its own stack rather than recursing: how deeply `parallelConfig`
/// nests is chosen by whoever created the plan, so recursion would let one request
/// grow the call stack.
fn collect_route53_health_checks(steps: &[Step], found: &mut BTreeSet<(String, String, String)>) {
    let mut pending: Vec<&Step> = steps.iter().collect();
    while let Some(step) = pending.pop() {
        match &step.execution_block_configuration {
            ExecutionBlockConfiguration::parallelConfig(nested) => {
                pending.extend(nested.steps.iter());
            }
            ExecutionBlockConfiguration::route53HealthCheckConfig(config) => {
                for record_set in config.record_sets.iter().flatten() {
                    if let Some(region) = record_set.region.as_ref() {
                        found.insert((
                            config.hosted_zone_id.clone(),
                            config.record_name.clone(),
                            region.clone(),
                        ));
                    }
                }
            }
            _ => {}
        }
    }
}

/// What the clock needs to know about one step of the workflow an execution runs,
/// captured when the execution is created so a later edit of the plan cannot change what
/// a running execution does.
///
/// It sits beside the modelled [`StepState`] projection (same order, same length): the
/// projection is what the API reports, this is what decides the next transition.
#[derive(Debug, Clone)]
pub struct PlannedStep {
    /// The step's name, as `StepState$name` reports it.
    pub name: String,
    /// `Some(window)` when the step is a manual-approval execution block, and the window
    /// its configuration gives the approval; `None` for every other block type, which
    /// drives another service's resources and so is one this emulator cannot perform.
    pub approval_window: Option<Duration>,
}

impl PlannedStep {
    /// The clock's plan for one step of a stored workflow.
    pub fn of(step: &Step) -> Self {
        let approval_window = match &step.execution_block_configuration {
            // A manual approval is the one execution block whose work is inside THIS
            // service: it waits for `ApprovePlanExecutionStep` and drives nothing else.
            ExecutionBlockConfiguration::executionApprovalConfig(config) => {
                Some(approval_window(config.timeout_minutes))
            }
            _ => None,
        };
        Self {
            name: step.name.clone(),
            approval_window,
        }
    }
}

/// How long a manual-approval step waits, from the block's own configuration.
///
/// AWS documents the window on the block itself
/// (`ExecutionApprovalConfiguration$timeoutMinutes`: "The timeout value specified for the
/// configuration", `@range(min: 1)`, `@default: 60`), so the configured value is what is
/// waited — this is not a duration the emulator chooses. A value the model does not admit
/// falls back to its documented default.
fn approval_window(timeout_minutes: i32) -> Duration {
    let minutes = u64::try_from(timeout_minutes)
        .ok()
        .filter(|minutes| *minutes >= 1)
        .unwrap_or(DEFAULT_APPROVAL_TIMEOUT_MINUTES);
    Duration::from_secs(minutes * 60)
}

/// One execution of a plan, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct StoredExecution {
    pub plan_arn: String,
    pub execution_id: String,
    /// The plan `version` this execution was started against.
    pub plan_version: String,
    pub comment: Option<String>,
    pub mode: String,
    pub action: String,
    pub execution_state: String,
    pub execution_region: String,
    pub activate_region: Option<String>,
    pub deactivate_region: Option<String>,
    pub recovery_execution_id: Option<String>,
    pub start_time: DateTime,
    pub end_time: Option<DateTime>,
    pub updated_at: DateTime,
    pub step_states: Vec<StepState>,
    /// What the clock needs to know about each of those steps, in the same order.
    pub planned_steps: Vec<PlannedStep>,
    /// The execution's events, oldest first, as `ListPlanExecutionEvents` reports them.
    /// Bounded by the step count of the workflow the execution runs plus the two
    /// lifecycle events, so it cannot grow per request.
    pub events: Vec<ExecutionEvent>,
    /// When the clock owes this execution its next transition; `None` once it owes none —
    /// because the execution has reached a state it never leaves, or because it is waiting
    /// for a call (an approval) rather than for the clock.
    pub outcome_due_at: Option<SystemTime>,
}

impl StoredExecution {
    /// Whether the execution has reached a state it never leaves, so the plan is free.
    pub fn is_terminal(&self) -> bool {
        TERMINAL_EXECUTION_STATES.contains(&self.execution_state.as_str())
    }

    /// The position of the step the execution is waiting for an approval decision on.
    pub fn step_awaiting_approval(&self) -> Option<usize> {
        self.step_states
            .iter()
            .position(|step| step.status.as_deref() == Some(STEP_PENDING_APPROVAL))
    }

    /// The position of the first step the clock has not reached yet.
    pub fn next_unreached_step(&self) -> Option<usize> {
        self.step_states
            .iter()
            .position(|step| step.status.as_deref() == Some(STEP_NOT_STARTED))
    }

    /// The position of the step called `name`, whatever state it is in.
    pub fn step_named(&self, name: &str) -> Option<usize> {
        self.step_states
            .iter()
            .position(|step| step.name.as_deref() == Some(name))
    }
}

/// What a `clientToken` recorded, so a replay of `StartPlanExecution` returns the first
/// execution and a reuse with different parameters is refused.
#[derive(Debug, Clone)]
pub struct TokenRecord {
    /// The request parameters the token was first used with.
    pub fingerprint: String,
    pub plan_arn: String,
    pub execution_id: String,
    /// Where this record sits in the FIFO that bounds the token cache.
    pub recorded_at: u64,
}

/// The ONE transition the clock owes an execution, decided from the step it stands on.
enum Advance {
    /// Stop at the manual-approval step at this position, for the window given.
    HoldForApproval { index: usize, window: Duration },
    /// The step at this position is an execution block this emulator cannot perform.
    UnsupportedStep(usize),
    /// The approval window of the step at this position has closed undecided.
    ApprovalWindowClosed(usize),
    /// No step is left to reach.
    WorkflowDone,
}

/// What [`ComAmazonawsArcregionswitchArcregionswitchState::apply_approval`] found when it
/// tried to decide a step, so `ApprovePlanExecutionStep` can answer the right modelled
/// failure for each case.
#[derive(Debug)]
pub enum ApprovalOutcome {
    /// The decision was applied.
    Applied,
    /// This plan holds no execution with that id.
    NoSuchExecution,
    /// The execution runs no step of that name.
    NoSuchStep,
    /// The step is not waiting for a decision; it reports `status`.
    NotAwaitingApproval { status: String },
}

/// Per-service state for com-amazonaws-arcregionswitch-arcregionswitch. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsArcregionswitchArcregionswitchState {
    /// Plans by ARN, at most [`MAX_PLANS`] of them. A `BTreeMap` so `ListPlans` pages in
    /// a stable order and a token can simply name the ARN to resume from.
    pub plans: BTreeMap<String, StoredPlan>,
    /// Executions by `(plan ARN, executionId)`, at most [`MAX_EXECUTIONS_PER_PLAN`] per
    /// plan.
    pub executions: BTreeMap<(String, String), StoredExecution>,
    /// `clientToken` -> the execution it started, at most [`MAX_CLIENT_TOKENS`] of them.
    pub client_tokens: BTreeMap<String, TokenRecord>,
    /// The FIFO order of `client_tokens`, as `recorded_at -> clientToken`, so the oldest
    /// record can be evicted in log time instead of by scanning the cache.
    client_token_order: BTreeMap<u64, String>,
    /// The clock-owed transitions still outstanding, as
    /// `(due instant, plan ARN, executionId)`. Sorted by deadline, so [`Self::settle`]
    /// touches only what is actually due.
    due_transitions: BTreeSet<(SystemTime, String, String)>,
    /// Monotonic seed for the ids this service mints and for the token FIFO.
    seed: u64,
}

impl ComAmazonawsArcregionswitchArcregionswitchState {
    /// The next value of this store's monotonic id seed.
    pub fn next_seed(&mut self) -> u64 {
        self.seed += 1;
        self.seed
    }

    /// Apply the state changes the clock already made due at `now`.
    ///
    /// Only clock-owed work: a transition that was owed at `now` even if nobody had
    /// called. EVERY handler runs this first — the reporting ones included, which is
    /// what keeps them from disagreeing with the mutating ones about what the current
    /// state is — and it costs only the transitions that are due, never a walk of the
    /// store.
    ///
    /// The transitions are the workflow's own progression, one step at a time (see
    /// [`Self::advance_execution`]): the clock reaches a step, and the step either stops
    /// the execution for a manual approval, or is one this emulator cannot perform and
    /// fails it. A transition that lands another one in the past (a long-expired approval
    /// window, say) is applied in the same pass, which is why this loop re-reads the
    /// index rather than taking a snapshot of it.
    pub fn settle(&mut self, now: SystemTime) {
        while let Some(next) = self.due_transitions.pop_first() {
            if next.0 > now {
                self.due_transitions.insert(next);
                break;
            }
            let (due, plan_arn, execution_id) = next;
            self.advance_execution(&(plan_arn, execution_id), due);
        }
    }

    /// Apply the ONE transition the clock owed `key` at `due`.
    ///
    /// The step the execution stands on decides which it is:
    ///
    /// * a step waiting for a manual approval whose window has closed →
    ///   [`PLAN_EXECUTION_TIMED_OUT`];
    /// * the next step not reached yet, when it IS a manual-approval block →
    ///   [`PENDING_MANUAL_APPROVAL`], and the clock then owes the window's end;
    /// * the next step not reached yet, when it is any other block → [`FAILED`], because
    ///   that block drives another service's resources and nothing performed it;
    /// * no step left to reach → [`COMPLETED`].
    fn advance_execution(&mut self, key: &(String, String), due: SystemTime) {
        let at = DateTime::from(due);
        let Some(execution) = self.executions.get(key) else {
            return;
        };
        let next = if execution.is_terminal() {
            // Nothing is owed to an execution that has reached a state it never leaves;
            // reaching here means a stale deadline outlived it, so it is dropped rather
            // than allowed to move a settled outcome.
            self.schedule(key, None);
            return;
        } else if let Some(index) = execution.step_awaiting_approval() {
            Advance::ApprovalWindowClosed(index)
        } else if let Some(index) = execution.next_unreached_step() {
            match execution
                .planned_steps
                .get(index)
                .and_then(|planned| planned.approval_window)
            {
                Some(window) => Advance::HoldForApproval { index, window },
                None => Advance::UnsupportedStep(index),
            }
        } else {
            // Every step of the workflow is done — or the workflow configures no
            // execution block at all, which has nothing to perform. Either way no
            // outcome is claimed for work nothing ran, because no work is left.
            Advance::WorkflowDone
        };

        match next {
            Advance::HoldForApproval { index, window } => {
                let name = self.mark_step(key, index, STEP_PENDING_APPROVAL, at, false);
                if let Some(execution) = self.executions.get_mut(key) {
                    execution.execution_state = PENDING_MANUAL_APPROVAL.to_string();
                    execution.updated_at = at;
                }
                self.push_detailed_event(key, EVENT_STEP_PENDING_APPROVAL, name, None, None, at);
                // The clock owes the window's end, not the outcome: until then this is a
                // call (`ApprovePlanExecutionStep`) the execution is waiting for.
                self.schedule(key, Some(due + window));
            }
            Advance::UnsupportedStep(index) => {
                self.mark_step(key, index, STEP_FAILED, at, true);
                if let Some(execution) = self.executions.get_mut(key) {
                    execution.execution_state = FAILED.to_string();
                    execution.updated_at = at;
                    execution.end_time = Some(at);
                }
                self.push_event(
                    key,
                    EVENT_EXECUTION_FAILED,
                    Some(UNSUPPORTED_EXECUTION_REASON.to_string()),
                    at,
                );
                self.finish_execution(key);
            }
            Advance::ApprovalWindowClosed(index) => {
                let name = self.mark_step(key, index, STEP_CANCELED, at, true);
                if let Some(execution) = self.executions.get_mut(key) {
                    execution.execution_state = PLAN_EXECUTION_TIMED_OUT.to_string();
                    execution.updated_at = at;
                    execution.end_time = Some(at);
                }
                self.push_detailed_event(
                    key,
                    EVENT_EXECUTION_FAILED,
                    name,
                    None,
                    Some(APPROVAL_TIMEOUT_REASON.to_string()),
                    at,
                );
                self.finish_execution(key);
            }
            Advance::WorkflowDone => {
                if let Some(execution) = self.executions.get_mut(key) {
                    execution.execution_state = COMPLETED.to_string();
                    execution.updated_at = at;
                    execution.end_time = Some(at);
                }
                self.push_event(key, EVENT_EXECUTION_SUCCEEDED, None, at);
                self.finish_execution(key);
            }
        }
    }

    /// Apply a manual-approval decision to one step of one execution.
    ///
    /// Approving releases the step — it reports [`STEP_COMPLETED`], the execution
    /// [`IN_PROGRESS`], and the clock owes the next step after [`EXECUTION_START_SPAN`],
    /// so a caller reading straight after the approval still sees the execution between
    /// steps. Declining stops the execution: the step is [`STEP_CANCELED`], the execution
    /// [`CANCELED`], and the plan is released. Only the step the execution is actually
    /// waiting on can be decided.
    pub fn apply_approval(
        &mut self,
        key: &(String, String),
        step_name: &str,
        approved: bool,
        comment: Option<String>,
        now: SystemTime,
    ) -> ApprovalOutcome {
        let Some(execution) = self.executions.get(key) else {
            return ApprovalOutcome::NoSuchExecution;
        };
        let Some(index) = execution.step_named(step_name) else {
            return ApprovalOutcome::NoSuchStep;
        };
        if execution.step_awaiting_approval() != Some(index) {
            let status = execution
                .step_states
                .get(index)
                .and_then(|step| step.status.clone())
                .unwrap_or_else(|| STEP_NOT_STARTED.to_string());
            return ApprovalOutcome::NotAwaitingApproval { status };
        }

        let at = DateTime::from(now);
        if approved {
            let name = self.mark_step(key, index, STEP_COMPLETED, at, true);
            if let Some(execution) = self.executions.get_mut(key) {
                execution.execution_state = IN_PROGRESS.to_string();
                execution.updated_at = at;
            }
            self.push_detailed_event(key, EVENT_STEP_SUCCEEDED, name, comment, None, at);
            self.schedule(key, Some(now + EXECUTION_START_SPAN));
        } else {
            let name = self.mark_step(key, index, STEP_CANCELED, at, true);
            if let Some(execution) = self.executions.get_mut(key) {
                execution.execution_state = CANCELED.to_string();
                execution.updated_at = at;
                execution.end_time = Some(at);
            }
            self.push_detailed_event(key, EVENT_EXECUTION_CANCELED, name, comment, None, at);
            self.finish_execution(key);
        }
        ApprovalOutcome::Applied
    }

    /// Move the step at `index` to `status`, timestamping it, and report its name.
    ///
    /// `startTime` is filled in only when the step carries none: a step the clock stopped
    /// at for an approval already records the instant it was reached.
    fn mark_step(
        &mut self,
        key: &(String, String),
        index: usize,
        status: &str,
        at: DateTime,
        finished: bool,
    ) -> Option<String> {
        let execution = self.executions.get_mut(key)?;
        let step = execution.step_states.get_mut(index)?;
        step.status = Some(status.to_string());
        step.start_time = step.start_time.or(Some(at));
        if finished {
            step.end_time = Some(at);
        }
        step.name.clone()
    }

    /// Record when the clock next owes `key` a transition — `None` for never — keeping
    /// the due-transition index and the execution's own `outcome_due_at` in step, so the
    /// two can never disagree about what is outstanding.
    fn schedule(&mut self, key: &(String, String), due: Option<SystemTime>) {
        let previous = self
            .executions
            .get(key)
            .and_then(|execution| execution.outcome_due_at);
        if let Some(previous) = previous {
            self.due_transitions
                .remove(&(previous, key.0.clone(), key.1.clone()));
        }
        if let Some(execution) = self.executions.get_mut(key) {
            execution.outcome_due_at = due;
        }
        if let Some(due) = due {
            self.due_transitions
                .insert((due, key.0.clone(), key.1.clone()));
        }
    }

    /// The execution has reached a state it never leaves: the clock owes it nothing more,
    /// and it no longer holds its plan — which becomes deletable and executable again.
    fn finish_execution(&mut self, key: &(String, String)) {
        self.schedule(key, None);
        if let Some(plan) = self.plans.get_mut(&key.0) {
            if plan.active_execution.as_ref() == Some(&key.1) {
                plan.active_execution = None;
            }
        }
    }

    /// Append one execution-level event to an execution's timeline, chained to the one
    /// before it.
    pub fn push_event(
        &mut self,
        key: &(String, String),
        event_type: &str,
        error: Option<String>,
        at: DateTime,
    ) {
        self.push_detailed_event(key, event_type, None, None, error, at);
    }

    /// Append one event, which may name the step it is about and carry a description.
    pub fn push_detailed_event(
        &mut self,
        key: &(String, String),
        event_type: &str,
        step_name: Option<String>,
        description: Option<String>,
        error: Option<String>,
        at: DateTime,
    ) {
        let event_id = format!(
            "{:016x}",
            self.next_seed().wrapping_mul(EXECUTION_ID_MULTIPLIER)
        );
        if let Some(execution) = self.executions.get_mut(key) {
            let previous_event_id = execution.events.last().map(|event| event.event_id.clone());
            execution.events.push(ExecutionEvent {
                timestamp: Some(at),
                r#type: Some(event_type.to_string()),
                step_name,
                execution_block_type: None,
                resources: None,
                error,
                description,
                event_id,
                previous_event_id,
            });
        }
    }

    /// Store `execution` against its plan, registering the transition the clock owes it
    /// and opening its event timeline.
    ///
    /// The caller must have made room with [`Self::room_for_execution`] first.
    pub fn insert_execution(&mut self, execution: StoredExecution) {
        let key = (execution.plan_arn.clone(), execution.execution_id.clone());
        let started_at = execution.start_time;
        if let Some(due) = execution.outcome_due_at {
            self.due_transitions
                .insert((due, key.0.clone(), key.1.clone()));
        }
        if let Some(plan) = self.plans.get_mut(&key.0) {
            plan.executions.push(key.1.clone());
            plan.active_execution = Some(key.1.clone());
        }
        self.executions.insert(key.clone(), execution);
        self.push_event(&key, EVENT_EXECUTION_PENDING, None, started_at);
    }

    /// Make room for one more execution of `plan_arn`, and report whether there is any.
    ///
    /// Under [`MAX_EXECUTIONS_PER_PLAN`] there is nothing to do. At it, the oldest
    /// execution that has reached a state it never leaves is dropped; if none has, there
    /// is no room and the caller refuses the request.
    pub fn room_for_execution(&mut self, plan_arn: &str) -> bool {
        // The common path reads the list by reference and allocates nothing; only a plan
        // at the ceiling pays for collecting its candidate ids.
        let at_ceiling = self
            .plans
            .get(plan_arn)
            .is_some_and(|plan| plan.executions.len() >= MAX_EXECUTIONS_PER_PLAN);
        if !at_ceiling {
            return true;
        }
        let recorded: Vec<String> = self
            .plans
            .get(plan_arn)
            .map(|plan| plan.executions.clone())
            .unwrap_or_default();
        let evictable = recorded.into_iter().find(|execution_id| {
            self.executions
                .get(&(plan_arn.to_string(), execution_id.clone()))
                .is_some_and(StoredExecution::is_terminal)
        });
        let Some(evictable) = evictable else {
            return false;
        };
        self.forget_execution(plan_arn, &evictable);
        true
    }

    /// Record `token` as having started `execution_id`, evicting the oldest record once
    /// the cache is at [`MAX_CLIENT_TOKENS`].
    pub fn record_client_token(
        &mut self,
        token: String,
        fingerprint: String,
        plan_arn: String,
        execution_id: String,
    ) {
        while self.client_tokens.len() >= MAX_CLIENT_TOKENS {
            let Some((_, evicted)) = self.client_token_order.pop_first() else {
                break;
            };
            self.client_tokens.remove(&evicted);
        }
        let recorded_at = self.next_seed();
        self.client_token_order.insert(recorded_at, token.clone());
        self.client_tokens.insert(
            token,
            TokenRecord {
                fingerprint,
                plan_arn,
                execution_id,
                recorded_at,
            },
        );
    }

    /// Drop one execution and everything that points at it.
    fn forget_execution(&mut self, plan_arn: &str, execution_id: &str) {
        let key = (plan_arn.to_string(), execution_id.to_string());
        if let Some(execution) = self.executions.remove(&key) {
            if let Some(due) = execution.outcome_due_at {
                self.due_transitions
                    .remove(&(due, key.0.clone(), key.1.clone()));
            }
        }
        if let Some(plan) = self.plans.get_mut(plan_arn) {
            plan.executions.retain(|id| id != execution_id);
            if plan.active_execution.as_deref() == Some(execution_id) {
                plan.active_execution = None;
            }
        }
        self.forget_tokens_for(|record| record.execution_id == execution_id);
    }

    /// Drop a plan and everything recorded against it, so nothing outlives the plan it
    /// belongs to.
    pub fn forget_plan(&mut self, plan_arn: &str) {
        self.plans.remove(plan_arn);
        self.executions.retain(|(arn, _), _| arn != plan_arn);
        self.due_transitions.retain(|(_, arn, _)| arn != plan_arn);
        self.forget_tokens_for(|record| record.plan_arn == plan_arn);
    }

    /// Drop every token record `doomed` selects, from the cache and from its FIFO.
    fn forget_tokens_for(&mut self, doomed: impl Fn(&TokenRecord) -> bool) {
        let mut orders = Vec::new();
        self.client_tokens.retain(|_, record| {
            if doomed(record) {
                orders.push(record.recorded_at);
                false
            } else {
                true
            }
        });
        for order in orders {
            self.client_token_order.remove(&order);
        }
    }
}

/// The base-36 digits a plan ARN's suffix is drawn from: `[a-z0-9]`, as `PlanArn`'s
/// pattern requires.
const BASE36_DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// How many distinct 6-character base-36 suffixes there are.
const PLAN_SUFFIX_SPACE: u64 = 36u64.pow(6);

/// Scrambles the seed so successive plans do not get visibly consecutive suffixes.
/// Odd and not a multiple of three, hence coprime with `36^6 = 2^12 * 3^12`, so
/// `seed -> suffix` is injective and two plans never collide on one suffix.
const PLAN_SUFFIX_MULTIPLIER: u64 = 2_654_435_761;

/// Scrambles the seed into an execution id. Odd, so multiplication modulo 2^64 is a
/// bijection and two executions never share an id.
const EXECUTION_ID_MULTIPLIER: u64 = 0x9E37_79B9_7F4A_7C15;

/// The 6-character suffix a plan ARN ends in, minted from the store's seed.
pub fn plan_arn_suffix(seed: u64) -> String {
    // `(seed % SPACE) * MULTIPLIER` is at most 2176782335 * 2654435761 < u64::MAX, so
    // the product is exact and the reduction below is a true modular multiplication.
    let mut value = (seed % PLAN_SUFFIX_SPACE) * PLAN_SUFFIX_MULTIPLIER % PLAN_SUFFIX_SPACE;
    let mut digits = [b'0'; 6];
    for slot in digits.iter_mut().rev() {
        *slot = BASE36_DIGITS[(value % 36) as usize];
        value /= 36;
    }
    String::from_utf8(digits.to_vec()).expect("base-36 digits are ASCII")
}

/// The ARN for a plan: `arn:aws:arc-region-switch::<account>:plan/<name>:<suffix>`, as
/// `PlanArn`'s pattern requires (the Region segment is empty — the plan is global).
pub fn plan_arn(name: &str, seed: u64) -> String {
    ::engine::emulator_arn(
        "arc-region-switch",
        "",
        &format!("plan/{name}:{}", plan_arn_suffix(seed)),
    )
}

/// The id for an execution: `<region>/<16 hex digits>`, the form `RecoveryExecutionId`'s
/// pattern gives a prior execution's id, minted from the store's seed.
pub fn execution_id(region: &str, seed: u64) -> String {
    format!(
        "{region}/{:016x}",
        seed.wrapping_mul(EXECUTION_ID_MULTIPLIER)
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsArcregionswitchArcregionswitchState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsArcregionswitchArcregionswitchState>)
}

/// The Region a request is scoped to: the Region of its SigV4 credential scope, or
/// [`DEFAULT_REGION`] when it carries none.
///
/// This service's plans are global (their ARN has an empty Region segment), so a
/// request's Region matters only to the `...InRegion` operations, which report what the
/// Region they were called in holds.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}
