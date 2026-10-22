// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice service.
//!
//! The whole service is one resource — the scaling plan — so this module owns the
//! stored record, the lifecycle clock that moves a plan out of its non-terminal
//! status, and the two value helpers (timestamps and pagination cursors) that more
//! than one handler in `src/ops/` reaches.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{ApplicationSource, ScalingInstruction};

/// The only scaling-plan version this service models.
///
/// `CreateScalingPlanResponse$ScalingPlanVersion` is documented as "This value is
/// always 1. Currently, you cannot have multiple scaling plan versions."
pub const ONLY_VERSION: i64 = 1;

/// Documented `MaxResults` bounds for both Describe operations: "This value can be
/// between 1 and 50. The default value is 50."
/// (`smithy_query(query="doc",shape="DescribeScalingPlansRequest",member="MaxResults")`).
pub const MAX_RESULTS_MIN: i32 = 1;
/// Upper bound of the documented `MaxResults` range; also its documented default.
pub const MAX_RESULTS_MAX: i32 = 50;

/// How long a scaling plan stays in a non-terminal status before the clock moves it on.
///
/// CHOSEN, not documented: AWS publishes no duration for `CreationInProgress` ->
/// `Active`, so this emulator uses a fixed span inside its 250-750 ms band. A caller
/// acting straight after `CreateScalingPlan` therefore still observes the transient
/// status and the state errors that go with it.
pub const SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The most scaling plans this emulator will hold at once, enforced through the
/// modelled `LimitExceededException` ("Your account exceeded a limit. This exception
/// is thrown when a per-account resource limit is exceeded.").
///
/// AWS enforces a per-account quota here but does not publish its value in the model,
/// so the NUMBER is chosen; what is modelled is that the store is bounded, so a caller
/// cannot grow it without limit.
pub const MAX_SCALING_PLANS: usize = 50;

/// `ScalingPlanStatusCode` value of a plan whose configuration is applied.
pub const STATUS_ACTIVE: &str = "Active";
/// `ScalingPlanStatusCode` value of a plan that is still being created.
pub const STATUS_CREATION_IN_PROGRESS: &str = "CreationInProgress";
/// `ScalingPlanStatusCode` value of a plan that is still being updated.
pub const STATUS_UPDATE_IN_PROGRESS: &str = "UpdateInProgress";
/// `ScalingPlanStatusCode` value of a plan that is still being deleted.
pub const STATUS_DELETION_IN_PROGRESS: &str = "DeletionInProgress";

/// `ScalingStatusCode` value of a resource whose scaling configuration is applied.
pub const SCALING_STATUS_ACTIVE: &str = "Active";
/// `ScalingStatusCode` value of a resource whose scaling configuration is not applied
/// yet — "the scaling plan is being created or the scaling configuration could not be
/// applied" (`ScalingPlanResource$ScalingStatusCode` doc).
pub const SCALING_STATUS_INACTIVE: &str = "Inactive";
/// `ScalingStatusCode` value of a resource whose configuration is only partly applied —
/// "because the scaling plan is being created or deleted" (same doc).
pub const SCALING_STATUS_PARTIALLY_ACTIVE: &str = "PartiallyActive";

/// What the clock owes a plan that is in a non-terminal status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingOutcome {
    /// The plan becomes `Active`.
    Active,
    /// The plan is removed from state.
    Removed,
}

/// A transition the clock already owes at `due_at`, whether or not anybody calls.
#[derive(Debug, Clone, Copy)]
pub struct PendingTransition {
    /// The instant the transition became (or becomes) due.
    pub due_at: SystemTime,
    /// What the plan looks like once it is applied.
    pub outcome: PendingOutcome,
}

/// One stored scaling plan: exactly the members the `ScalingPlan` output shape
/// requires, so `DescribeScalingPlans` is a pure projection of this record.
#[derive(Debug, Clone)]
pub struct ScalingPlanRecord {
    /// `ScalingPlanName` — the primary key.
    pub name: String,
    /// `ScalingPlanVersion`; always [`ONLY_VERSION`].
    pub version: i64,
    /// `ApplicationSource`, stored as the caller supplied it.
    pub application_source: ApplicationSource,
    /// `ScalingInstructions`, stored as the caller supplied them.
    pub scaling_instructions: Vec<ScalingInstruction>,
    /// `StatusCode` — a `ScalingPlanStatusCode` value.
    pub status_code: String,
    /// `StatusMessage`.
    pub status_message: Option<String>,
    /// `StatusStartTime` — when the plan entered `status_code`.
    pub status_start_time: SystemTime,
    /// `CreationTime`.
    pub creation_time: SystemTime,
    /// The clock-owed transition out of a non-terminal `status_code`, if any.
    pub pending: Option<PendingTransition>,
}

impl ScalingPlanRecord {
    /// A freshly created plan: `CreationInProgress`, with the move to `Active` already
    /// owed at `now + SETTLE_AFTER`.
    pub fn creating(
        name: String,
        application_source: ApplicationSource,
        scaling_instructions: Vec<ScalingInstruction>,
        now: SystemTime,
    ) -> Self {
        Self {
            name,
            version: ONLY_VERSION,
            application_source,
            scaling_instructions,
            status_code: STATUS_CREATION_IN_PROGRESS.to_string(),
            status_message: None,
            status_start_time: now,
            creation_time: now,
            pending: Some(PendingTransition {
                due_at: now + SETTLE_AFTER,
                outcome: PendingOutcome::Active,
            }),
        }
    }

    /// Enter a non-terminal `status`, owing `outcome` at `now + SETTLE_AFTER`.
    pub fn begin(&mut self, status: &str, outcome: PendingOutcome, now: SystemTime) {
        self.status_code = status.to_string();
        self.status_start_time = now;
        self.pending = Some(PendingTransition {
            due_at: now + SETTLE_AFTER,
            outcome,
        });
    }

    /// True while the plan is being created, updated or deleted — the state in which
    /// `UpdateScalingPlan` and `DeleteScalingPlan` answer `ConcurrentUpdateException`
    /// ("You cannot update a scaling plan if it is in the process of being created,
    /// updated, or deleted").
    pub fn is_in_progress(&self) -> bool {
        self.pending.is_some()
    }

    /// The `ScalingStatusCode` every resource of this plan reports, derived from the
    /// plan's own status: applied only once the plan itself is applied.
    pub fn scaling_status_code(&self) -> &'static str {
        match self.status_code.as_str() {
            STATUS_ACTIVE => SCALING_STATUS_ACTIVE,
            STATUS_CREATION_IN_PROGRESS => SCALING_STATUS_INACTIVE,
            _ => SCALING_STATUS_PARTIALLY_ACTIVE,
        }
    }
}

/// Per-service state for com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAutoscalingplansAnyscalescalingplannerfrontendserviceState {
    /// Every stored plan, keyed by `ScalingPlanName`. A `BTreeMap` so both Describe
    /// operations page over a stable, deterministic ordering.
    pub scaling_plans: BTreeMap<String, ScalingPlanRecord>,
}

impl ComAmazonawsAutoscalingplansAnyscalescalingplannerfrontendserviceState {
    /// Apply every transition the clock already owes at `now`.
    ///
    /// This is the ONLY write a reporting operation makes: the work was due at `now`
    /// whether or not anybody called, so running it here cannot make a read change
    /// what a later caller sees.
    pub fn settle_due(&mut self, now: SystemTime) {
        let due: Vec<(String, PendingOutcome, SystemTime)> = self
            .scaling_plans
            .values()
            .filter_map(|plan| {
                let pending = plan.pending?;
                (pending.due_at <= now)
                    .then(|| (plan.name.clone(), pending.outcome, pending.due_at))
            })
            .collect();
        for (name, outcome, due_at) in due {
            match outcome {
                PendingOutcome::Removed => {
                    self.scaling_plans.remove(&name);
                }
                PendingOutcome::Active => {
                    if let Some(plan) = self.scaling_plans.get_mut(&name) {
                        plan.status_code = STATUS_ACTIVE.to_string();
                        // The plan entered `Active` when the transition came due, not
                        // when somebody happened to look.
                        plan.status_start_time = due_at;
                        plan.pending = None;
                    }
                }
            }
        }
    }

    /// The plan stored under `name` whose version is `version`.
    pub fn plan(&self, name: &str, version: i64) -> Option<&ScalingPlanRecord> {
        self.scaling_plans
            .get(name)
            .filter(|plan| plan.version == version)
    }

    /// Mutable access to the plan stored under `name` whose version is `version`.
    pub fn plan_mut(&mut self, name: &str, version: i64) -> Option<&mut ScalingPlanRecord> {
        self.scaling_plans
            .get_mut(name)
            .filter(|plan| plan.version == version)
    }
}

/// A modelled `TimestampType` from a stored wall-clock instant.
///
/// Clamped at the epoch rather than panicking: a host clock before 1970 is not a
/// client error.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The identity of an `ApplicationSource`, used to enforce "You can create one
/// scaling plan per application source" (`ApplicationSource` member doc).
///
/// `None` when the source names neither a stack nor a tag filter, so it identifies no
/// application at all. A source that names BOTH is refused before this is reached —
/// see [`validate_application_source`].
pub fn application_source_key(source: &ApplicationSource) -> Option<String> {
    if let Some(arn) = source
        .cloud_formation_stack_arn
        .as_deref()
        .filter(|arn| !arn.is_empty())
    {
        return Some(format!("stack:{arn}"));
    }
    let filters = source.tag_filters.as_ref()?;
    if filters.is_empty() {
        return None;
    }
    let mut rendered: Vec<String> = filters
        .iter()
        .map(|filter| {
            let key = filter.key.clone().unwrap_or_default();
            let mut values = filter.values.clone().unwrap_or_default();
            values.sort();
            format!("{key}={}", values.join(","))
        })
        .collect();
    rendered.sort();
    Some(format!("tags:{}", rendered.join(";")))
}

/// Whether a tag-filter list identifies anything: a filter list that is absent or
/// empty selects nothing.
fn names_tag_filters(source: &ApplicationSource) -> bool {
    source
        .tag_filters
        .as_ref()
        .is_some_and(|filters| !filters.is_empty())
}

/// Whether a `CloudFormationStackARN` is present and non-empty.
fn names_stack(source: &ApplicationSource) -> bool {
    source
        .cloud_formation_stack_arn
        .as_deref()
        .is_some_and(|arn| !arn.is_empty())
}

/// Validate an `ApplicationSource` a caller wrote, returning its identity key.
///
/// `ApplicationSource` is documented as "A CloudFormation stack or a set of tags" —
/// the two are ALTERNATIVES, so naming neither identifies no application and naming
/// both is contradictory. `Err` carries the refusal message naming the members.
pub fn validate_application_source(source: &ApplicationSource) -> Result<String, &'static str> {
    match (names_stack(source), names_tag_filters(source)) {
        (true, true) => Err(
            "ApplicationSource must specify either CloudFormationStackARN or TagFilters, not both",
        ),
        (false, false) => Err(
            "ApplicationSource must specify either CloudFormationStackARN or TagFilters",
        ),
        _ => application_source_key(source).ok_or(
            "ApplicationSource must specify either CloudFormationStackARN or TagFilters",
        ),
    }
}

/// AWS's wording for "the name and version in this request name no stored plan",
/// built from the request's own `ScalingPlanName` and `ScalingPlanVersion`.
///
/// Every operation that looks a plan up by name and version refuses with this one
/// text, whatever error shape it models the refusal in (`ObjectNotFoundException`
/// where the operation models it, `ValidationException` where it does not), so a
/// client that discriminates not-found variants by message reads the same sentence
/// on every path.
pub fn plan_not_found_message(name: &str, version: i64) -> String {
    format!("No scaling plan found for scaling plan name: {name}, version: {version}")
}

/// Encode a pagination cursor as an opaque token.
///
/// The `kind` prefix is part of the encoded value, so a token minted by one
/// operation does not decode for another.
pub fn encode_cursor(kind: &str, value: &str) -> String {
    hex::encode(format!("{kind}:{value}"))
}

/// Decode a cursor minted by [`encode_cursor`] for the same `kind`.
///
/// `None` for anything this service did not mint — the caller answers
/// `InvalidNextTokenException`.
pub fn decode_cursor(kind: &str, token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    decoded
        .strip_prefix(kind)
        .and_then(|rest| rest.strip_prefix(':'))
        .map(ToString::to_string)
}

/// The page size to use, honouring the documented 1..=50 range and the documented
/// default of 50.
///
/// `Err` carries the refusal message naming the member and the constraint it broke.
pub fn effective_max_results(requested: Option<i32>) -> Result<usize, String> {
    match requested {
        None => Ok(MAX_RESULTS_MAX as usize),
        Some(value) if (MAX_RESULTS_MIN..=MAX_RESULTS_MAX).contains(&value) => Ok(value as usize),
        Some(value) => Err(format!(
            "1 validation error detected: Value '{value}' at 'maxResults' failed to satisfy \
             constraint: Member must be between {MAX_RESULTS_MIN} and {MAX_RESULTS_MAX}"
        )),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAutoscalingplansAnyscalescalingplannerfrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAutoscalingplansAnyscalescalingplannerfrontendserviceState>)
}
