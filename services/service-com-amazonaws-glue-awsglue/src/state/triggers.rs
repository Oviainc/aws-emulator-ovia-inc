// SPDX-License-Identifier: CC0-1.0

//! The stored trigger record, the comparisons that decide whether an update names a change,
//! and the refusals a trigger definition earns before it is stored.

use super::*;

/// A stored trigger: the `Trigger` every read projects, plus the `Tags` the create
/// carried, which that shape does not hold.
///
/// Every member of the modelled `Trigger` shape is held here except `Id`, which the model
/// states is "Reserved for future use" and which therefore has no value to report. The one
/// member the server sets is [`Self::state`]: a client names a trigger's type, schedule,
/// actions and predicate, and whether the create should start it, and the service decides
/// the state those leave it in.
///
/// Tags are kept on the record so they are not lost by the create, as a crawler's and a
/// custom entity type's are; [`TaggedResourceKind`] carries no trigger variant yet, so
/// resolving one through the tagging operations belongs to those operations' own issues.
#[derive(Debug, Clone)]
pub struct TriggerEntry {
    /// `Trigger$Name`, which is also this entry's key in its Region's map inside
    /// [`ComAmazonawsGlueAwsglueState::triggers`].
    pub name: String,
    /// `Trigger$Type`: one of the modelled `TriggerType` values, as the create sent it.
    pub trigger_type: String,
    /// `Trigger$State`: one of the modelled `TriggerState` values, decided by the service
    /// rather than sent — [`TRIGGER_STATE_ACTIVATED`] when the create asked for the
    /// trigger to be started, else [`TRIGGER_STATE_CREATED`].
    pub state: String,
    /// `Trigger$WorkflowName`: "The name of the workflow associated with the trigger",
    /// stored as sent.
    pub workflow_name: Option<String>,
    pub description: Option<String>,
    /// `Trigger$Schedule`: the cron expression the create supplied, if any.
    pub schedule: Option<String>,
    /// `Trigger$Actions`: "The actions initiated by this trigger."
    pub actions: Vec<Action>,
    /// `Trigger$Predicate`: "The predicate of this trigger, which defines when it will
    /// fire", absent for a trigger created without one.
    pub predicate: Option<Predicate>,
    pub event_batching_condition: Option<EventBatchingCondition>,
    /// How many events EventBridge has delivered to this trigger's workflow since its
    /// current batch opened, and `0` when no batch is open.
    ///
    /// Server-set and reported by no member of the `Trigger` shape: it is the accounting
    /// the `EventBatchingCondition` is read against — "Number of events that must be
    /// received from Amazon EventBridge before EventBridge event trigger fires"
    /// ([`EventBatchingCondition$BatchSize`]) — so what a client observes of it is the
    /// workflow run the batch starts and not the count itself. See [`Self::receive_event`].
    ///
    /// [`EventBatchingCondition$BatchSize`]: EventBatchingCondition
    pub batched_events: i32,
    /// When this trigger's current batch opened, and `None` when no batch is open:
    /// "Window starts when first event is received" (`EventBatchingCondition$BatchWindow`),
    /// so this is the instant the FIRST event of the open batch arrived and the window is
    /// measured from it.
    pub batch_window_opened_at: Option<SystemTime>,
    /// The request's `StartOnCreation`, as the create that stored this record sent it.
    ///
    /// Kept beside the definition because it is a member of the REQUEST rather than of the
    /// `Trigger` shape — no read reports it — and because [`Self::state`] is no record of
    /// it: a later start or stop moves the state, so what the create asked for would be
    /// lost. It is what the idempotency comparison reads
    /// ([`Self::matches_create_input`]).
    pub start_on_creation: bool,
    /// The request's `Tags`, which are not part of the `Trigger` shape.
    pub tags: HashMap<String, String>,
}

impl TriggerEntry {
    /// Build a stored trigger from a `CreateTrigger` request, in the state the create
    /// leaves it in.
    ///
    /// `state` is that state, decided by the handler from the request's `StartOnCreation`
    /// and `Type` — the one member of the record the client does not set.
    pub fn from_create_input(input: &CreateTriggerInput, state: &str) -> Self {
        Self {
            name: input.name.clone(),
            trigger_type: input.r#type.clone(),
            state: state.to_string(),
            workflow_name: input.workflow_name.clone(),
            description: input.description.clone(),
            schedule: input.schedule.clone(),
            actions: input.actions.clone(),
            predicate: input.predicate.clone(),
            event_batching_condition: input.event_batching_condition.clone(),
            // A trigger a create registered has been handed no event yet, so no batch of
            // one is open.
            batched_events: 0,
            batch_window_opened_at: None,
            start_on_creation: input.start_on_creation,
            tags: input.tags.clone().unwrap_or_default(),
        }
    }

    /// Whether this stored trigger is the one `input` asks to create, member for member.
    ///
    /// A trigger is addressed by its name, so the name is what a create reuses when it
    /// submits a definition under one already taken: such a create is a RETRY of the create
    /// that stored this record only when it asks for the very same trigger, and otherwise it
    /// has associated one identifier with two different records. The comparison is over
    /// every member a create sets, the tags included, because any of them is part of the
    /// trigger the caller asked for.
    pub fn matches_create_input(&self, input: &CreateTriggerInput) -> bool {
        // The name is compared too, so the comparison is member for member and holds for
        // any pair rather than only for a lookup already keyed by the requested name.
        self.name == input.name
            && self.trigger_type == input.r#type
            // The REQUEST member, not the trigger's state, which a later start or stop
            // moves: an identical retry is still a retry of a trigger that has been
            // started since.
            && self.start_on_creation == input.start_on_creation
            && self.workflow_name == input.workflow_name
            && self.description == input.description
            && self.schedule == input.schedule
            && same_trigger_actions(&self.actions, &input.actions)
            && same_trigger_predicate(self.predicate.as_ref(), input.predicate.as_ref())
            && same_event_batching_condition(
                self.event_batching_condition.as_ref(),
                input.event_batching_condition.as_ref(),
            )
            && input
                .tags
                .as_ref()
                .map_or(self.tags.is_empty(), |tags| &self.tags == tags)
    }

    /// Whether this trigger CAN START the job `job_name`, i.e. whether firing it would
    /// begin a run of that job.
    ///
    /// A trigger initiates "the actions initiated by this trigger" (`Trigger$Actions`) and
    /// an action naming a job begins a run of it ([`ComAmazonawsGlueAwsglueState::
    /// initiate_trigger_actions`]), so the jobs a trigger can start are exactly the
    /// `JobName`s of its actions. The job a condition WAITS ON (`Condition$JobName`) is not
    /// one the trigger starts, so a predicate does not qualify a trigger here. Names are
    /// compared exactly, as every other read of a job name is.
    pub fn starts_job(&self, job_name: &str) -> bool {
        self.actions
            .iter()
            .any(|action| action.job_name.as_deref() == Some(job_name))
    }

    /// Move this trigger into the state a start leaves it in.
    ///
    /// "Starts an existing trigger" (`StartTrigger`) ARMS a trigger that waits on something
    /// outside the caller — its schedule, its predicate or the events it batches — so such a
    /// trigger is left in [`TRIGGER_STATE_ACTIVATED`], which every later read reports until
    /// a stop moves it.
    ///
    /// A trigger that waits on nothing but a caller has no armed state to be put into: it is
    /// the one type a create cannot start, "Set to true to start SCHEDULED and CONDITIONAL
    /// triggers when created. True is not supported for ON_DEMAND triggers"
    /// (`CreateTriggerRequest$StartOnCreation`), because there is nothing for it to wait for
    /// once armed. Starting one FIRES it, and what the start leaves it in is the state it
    /// waits in ([`TRIGGER_STATE_CREATED`]), ready for the next caller.
    pub fn start(&mut self) {
        self.state = if self.trigger_type == TRIGGER_TYPE_ON_DEMAND {
            TRIGGER_STATE_CREATED.to_string()
        } else {
            TRIGGER_STATE_ACTIVATED.to_string()
        };
    }

    /// Replace the stored DEFINITION with the one a `TriggerUpdate` carries: "this object
    /// updates the previous trigger definition by overwriting it completely"
    /// (`TriggerUpdate`), so a member the update omits is left with no value rather than
    /// with the one the previous definition had.
    ///
    /// Only the five definition members `TriggerUpdate` declares are touched. The record's
    /// identity and the members the shape carries no value for — [`Self::name`],
    /// [`Self::trigger_type`], [`Self::state`], [`Self::workflow_name`] and [`Self::tags`]
    /// — are left as they are; `TriggerUpdate$Name` is "Reserved for future use", so it
    /// renames nothing.
    pub fn apply_update(&mut self, update: &TriggerUpdate) {
        self.description = update.description.clone();
        self.schedule = update.schedule.clone();
        self.actions = update.actions.clone().unwrap_or_default();
        self.predicate = update.predicate.clone();
        self.event_batching_condition = update.event_batching_condition.clone();
        // The batch open under the PREVIOUS condition was counted against the batch size
        // and window that condition named, and this update overwrites both, so it is
        // closed rather than carried over: the next event delivered opens a batch under
        // the condition now in force.
        self.close_event_batch();
    }

    /// Whether EventBridge deliveries to this trigger's workflow are counted against it.
    ///
    /// Only an `EVENT` trigger batches events — every other `TriggerType` fires on a
    /// caller, the clock or a predicate — and a trigger a stop deactivated fires on
    /// nothing at all ("Stops a specified trigger", `StopTrigger`), so a delivery is
    /// counted against neither.
    pub fn batches_events(&self) -> bool {
        self.trigger_type == TRIGGER_TYPE_EVENT && self.state != TRIGGER_STATE_DEACTIVATED
    }

    /// How many events have to arrive before this trigger fires: "Number of events that
    /// must be received from Amazon EventBridge before EventBridge event trigger fires"
    /// (`EventBatchingCondition$BatchSize`), and [`DEFAULT_EVENT_BATCH_SIZE`] for a trigger
    /// carrying no condition.
    ///
    /// Held at the member's lower bound (`@range 1..100`), so a stored zero cannot make the
    /// trigger unfireable — the same floor a concurrency ceiling gets.
    pub fn event_batch_size(&self) -> i32 {
        self.event_batching_condition
            .as_ref()
            .map_or(DEFAULT_EVENT_BATCH_SIZE, |condition| {
                condition.batch_size.max(DEFAULT_EVENT_BATCH_SIZE)
            })
    }

    /// How long this trigger's open batch waits for the rest of its events: "Window of
    /// time in seconds after which EventBridge event trigger fires"
    /// (`EventBatchingCondition$BatchWindow`), and
    /// [`DEFAULT_EVENT_BATCH_WINDOW_SECONDS`] for a condition that names none.
    ///
    /// Held at the member's lower bound of one second for the reason
    /// [`Self::event_batch_size`] gives.
    pub fn event_batch_window(&self) -> Duration {
        let seconds = self
            .event_batching_condition
            .as_ref()
            .and_then(|condition| condition.batch_window)
            .and_then(|window| u64::try_from(window).ok())
            .unwrap_or(DEFAULT_EVENT_BATCH_WINDOW_SECONDS);
        Duration::from_secs(seconds.max(1))
    }

    /// Count ONE event EventBridge delivered to this trigger's workflow, and answer
    /// whether the batch is now COMPLETE — i.e. whether the trigger fires.
    ///
    /// "Batch condition that must be met (specified number of events received or batch time
    /// window expired) before EventBridge event trigger fires"
    /// (`EventBatchingCondition`): this is the count half of that condition, so the batch
    /// completes on the delivery that reaches [`Self::event_batch_size`] and the trigger
    /// fires ONCE for it, however many events the batch coalesced. A delivery that does not
    /// complete the batch fires nothing, which is what setting a batch size is FOR.
    ///
    /// The first event of a batch also opens its window, because "window starts when first
    /// event is received" ([`Self::batch_window_due_at`]); a completed batch closes, so the
    /// next event opens a new one.
    pub fn receive_event(&mut self, now: SystemTime) -> bool {
        self.batched_events = self.batched_events.saturating_add(1);
        if self.batch_window_opened_at.is_none() {
            self.batch_window_opened_at = Some(now);
        }
        if self.batched_events < self.event_batch_size() {
            return false;
        }
        self.close_event_batch();
        true
    }

    /// When the clock fires this trigger's open batch on its own, and `None` when no batch
    /// is open: the instant its window runs out, which is the window measured from the
    /// first event the batch received.
    ///
    /// The due instant is STORED (as the window's opening instant) rather than waited for:
    /// the firing it owes is materialised by whichever later call reads the workflow's runs
    /// ([`ComAmazonawsGlueAwsglueState::settle_due_event_batches`]), so nothing sleeps.
    pub fn batch_window_due_at(&self) -> Option<SystemTime> {
        if self.batched_events <= 0 {
            return None;
        }
        Some(self.batch_window_opened_at? + self.event_batch_window())
    }

    /// Whether the clock has already run this trigger's open batch window out at `now`, so
    /// the batch fires with the events it has.
    pub fn batch_window_expired(&self, now: SystemTime) -> bool {
        self.batch_window_due_at().is_some_and(|due| now >= due)
    }

    /// Close the open batch: the events it held have fired the trigger (or the window they
    /// were waiting in has), so the next delivery opens a new batch and a new window.
    pub(super) fn close_event_batch(&mut self) {
        self.batched_events = 0;
        self.batch_window_opened_at = None;
    }

    /// Rebuild the modelled `Trigger` output shape from what is stored.
    pub fn to_trigger(&self) -> Trigger {
        Trigger {
            name: Some(self.name.clone()),
            workflow_name: self.workflow_name.clone(),
            // "Reserved for future use": a trigger carries no id of its own, so there is
            // none to report.
            id: None,
            r#type: Some(self.trigger_type.clone()),
            state: Some(self.state.clone()),
            description: self.description.clone(),
            schedule: self.schedule.clone(),
            actions: Some(self.actions.clone()),
            predicate: self.predicate.clone(),
            event_batching_condition: self.event_batching_condition.clone(),
        }
    }
}

/// What became of ONE event EventBridge delivered to a Glue workflow
/// ([`ComAmazonawsGlueAwsglueState::notify_workflow_event`]).
///
/// The delivery is the `glue:notifyEvent` ingress, so this says whether the event was
/// counted into a batch that has not filled yet, whether it completed the batch and began a
/// run, and why no batch could be counted at all — which is what lets the caller that made
/// the delivery report a handover the workflow accepted apart from one it could not make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowEventDelivery {
    /// The event was counted against the trigger's batch, which is not complete yet:
    /// `received` of the `batch_size` events the condition asks for have arrived, so
    /// nothing has fired.
    Counted {
        /// How many events the open batch now holds.
        received: i32,
        /// The `BatchSize` the batch is counted against
        /// ([`TriggerEntry::event_batch_size`]).
        batch_size: i32,
    },
    /// The event completed the batch, and the trigger fired: this run of the workflow
    /// began.
    Fired {
        /// The `WorkflowRunId` of the run the firing began.
        run_id: String,
    },
    /// The workflow's EVENT trigger is stopped, so nothing counts the event and nothing
    /// fires — the delivery itself was still accepted.
    Deactivated,
    /// This account holds no workflow of that name, so there is nothing to deliver to.
    NoWorkflow,
    /// The workflow holds no EVENT trigger in this Region, so no batch exists for the event
    /// to be counted against.
    NoEventTrigger,
    /// The batch completed, but the workflow is already running as many runs as its
    /// `MaxConcurrentRuns` allows, so the firing began none.
    ConcurrentRunsExceeded {
        /// The ceiling the workflow named.
        ceiling: i32,
    },
}

/// Whether two action lists are the same actions, in the same order. The modelled `Action`
/// derives no equality, so its members are compared here, as
/// [`same_data_quality_target_table`] compares its own shape's.
pub(super) fn same_trigger_actions(stored: &[Action], requested: &[Action]) -> bool {
    stored.len() == requested.len()
        && stored
            .iter()
            .zip(requested.iter())
            .all(|(stored, requested)| {
                stored.job_name == requested.job_name
                    && stored.crawler_name == requested.crawler_name
                    && stored.arguments == requested.arguments
                    && stored.timeout == requested.timeout
                    && stored.security_configuration == requested.security_configuration
                    && same_notification_property(
                        stored.notification_property.as_ref(),
                        requested.notification_property.as_ref(),
                    )
            })
}

/// Whether two optional `NotificationProperty` values are the same delay.
pub(super) fn same_notification_property(
    stored: Option<&NotificationProperty>,
    requested: Option<&NotificationProperty>,
) -> bool {
    match (stored, requested) {
        (None, None) => true,
        (Some(stored), Some(requested)) => {
            stored.notify_delay_after == requested.notify_delay_after
        }
        _ => false,
    }
}

/// Whether two optional `Predicate` values are the same predicate: the same `Logical` over
/// the same conditions, in the same order.
pub(super) fn same_trigger_predicate(stored: Option<&Predicate>, requested: Option<&Predicate>) -> bool {
    match (stored, requested) {
        (None, None) => true,
        (Some(stored), Some(requested)) => {
            stored.logical == requested.logical
                && same_trigger_conditions(
                    stored.conditions.as_deref(),
                    requested.conditions.as_deref(),
                )
        }
        _ => false,
    }
}

/// Whether two optional condition lists are the same conditions, in the same order.
pub(super) fn same_trigger_conditions(stored: Option<&[Condition]>, requested: Option<&[Condition]>) -> bool {
    match (stored, requested) {
        (None, None) => true,
        (Some(stored), Some(requested)) => {
            stored.len() == requested.len()
                && stored
                    .iter()
                    .zip(requested.iter())
                    .all(|(stored, requested)| {
                        stored.logical_operator == requested.logical_operator
                            && stored.job_name == requested.job_name
                            && stored.state == requested.state
                            && stored.crawler_name == requested.crawler_name
                            && stored.crawl_state == requested.crawl_state
                    })
        }
        _ => false,
    }
}

/// Whether two optional `EventBatchingCondition` values are the same batch condition.
pub(super) fn same_event_batching_condition(
    stored: Option<&EventBatchingCondition>,
    requested: Option<&EventBatchingCondition>,
) -> bool {
    match (stored, requested) {
        (None, None) => true,
        (Some(stored), Some(requested)) => {
            stored.batch_size == requested.batch_size
                && stored.batch_window == requested.batch_window
        }
        _ => false,
    }
}

/// One Glue entity a trigger's definition names: the workflow it belongs to, the job an
/// action runs or a condition waits on, or the crawler of either.
///
/// Returned by [`missing_trigger_reference`], so a refusal can say which kind of entity it
/// could not find and under which name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerReference<'a> {
    /// `Action$JobName` or `Condition$JobName`.
    Job(&'a str),
    /// `Action$CrawlerName` or `Condition$CrawlerName`.
    Crawler(&'a str),
    /// `CreateTriggerRequest$WorkflowName`.
    Workflow(&'a str),
}

impl TriggerReference<'_> {
    /// How a trigger write refuses the definition that names this missing entity: the
    /// wording the lookup of THAT kind of entity answers with wherever it is made, so a
    /// create and an update report one missing job, crawler or workflow alike.
    pub fn refusal_message(&self) -> String {
        match self {
            // The job-definition miss is the same fixed sentence wherever a job is looked
            // up, so a trigger that would run a job nothing holds is refused with it too,
            // and discloses no name.
            Self::Job(_) => messages::JOB_NOT_FOUND.to_string(),
            Self::Crawler(name) => messages::crawler_not_found(name),
            // The workflow miss is the same fixed sentence wherever a workflow is looked
            // up, so a trigger that would belong to a workflow nothing holds is refused
            // with it too, and discloses no name.
            Self::Workflow(_) => messages::WORKFLOW_NOT_FOUND.to_string(),
        }
    }
}

/// The first rule the trigger DEFINITION `trigger_type`/`schedule`/`actions`/`predicate`
/// breaks, worded as the refusal to answer with, or `None` when the definition is one the
/// service could fire.
///
/// One copy of the rule, for every write that leaves a definition behind: `CreateTrigger`
/// stores the definition its request carries and `UpdateTrigger` overwrites a stored one
/// with the definition its `TriggerUpdate` carries, and both have to end up with a trigger
/// that knows what to initiate and when. Every rule here is a modelled requirement:
/// "the actions initiated by this trigger" (`Trigger$Actions`), "this field is required
/// when the trigger type is SCHEDULED" (`Schedule`) and "...is CONDITIONAL" (`Predicate`),
/// "an optional field if only one condition is listed. If multiple conditions are listed,
/// then this field is required" (`Predicate$Logical`), and "a `cron` expression used to
/// specify the schedule" (`Schedule`).
///
/// The caller lifts the sentence into its OWN modelled input error, which is the only part
/// of the refusal that differs between the operations.
pub fn trigger_definition_refusal(
    trigger_type: &str,
    schedule: Option<&str>,
    actions: &[Action],
    predicate: Option<&Predicate>,
) -> Option<String> {
    // A trigger that names no action initiates nothing when it fires, so a definition
    // without one is refused rather than stored.
    if actions.is_empty() {
        return Some(messages::trigger_member_required("Actions"));
    }

    // Without the member its type requires, the service would hold a trigger with nothing
    // to decide when it fires.
    if trigger_type == TRIGGER_TYPE_SCHEDULED && schedule.is_none() {
        return Some(messages::trigger_member_required("Schedule"));
    }
    let conditions = predicate
        .and_then(|predicate| predicate.conditions.as_ref())
        .map_or(0, Vec::len);
    if trigger_type == TRIGGER_TYPE_CONDITIONAL && conditions == 0 {
        return Some(messages::trigger_member_required("Predicate"));
    }

    // Without a `Logical`, a multi-condition predicate does not say whether all or any of
    // its conditions must hold.
    if conditions > 1 && predicate.is_none_or(|predicate| predicate.logical.is_none()) {
        return Some(messages::predicate_logical_required(conditions));
    }

    // Anything but a `cron(<six fields>)` expression is not a schedule this service can
    // run, so it is refused rather than stored and reported back.
    if let Some(schedule) = schedule {
        if !is_cron_expression(schedule) {
            return Some(messages::schedule_not_a_cron_expression(schedule));
        }
    }

    // A conditional trigger fires the actions it carries when the conditions it watches
    // are met, so a job that is BOTH what the trigger waits on and what it runs describes
    // a trigger that would fire on its own output: the definition is refused rather than
    // stored, whether or not that job exists.
    if trigger_type == TRIGGER_TYPE_CONDITIONAL && job_is_predicate_and_action(actions, predicate)
    {
        return Some(messages::JOB_IN_PREDICATE_AND_ACTION.to_string());
    }

    None
}

/// Whether one job is named both by a condition of `predicate` and by an action of
/// `actions` — the overlap a conditional trigger's definition may not carry.
///
/// A job is named exactly as the request spells it (job definitions are keyed by their
/// name, unfolded), so the comparison is over those names as sent. The condition names are
/// collected into a SET first and each action is then one probe of it, so the work is the
/// number of conditions PLUS the number of actions rather than their product: neither
/// factor, both of which the one request body carries, can multiply the other. A definition
/// that names no job in one of the two places cannot overlap at all.
pub(super) fn job_is_predicate_and_action(actions: &[Action], predicate: Option<&Predicate>) -> bool {
    let condition_jobs: BTreeSet<&str> = predicate
        .and_then(|predicate| predicate.conditions.as_ref())
        .into_iter()
        .flatten()
        .filter_map(|condition| condition.job_name.as_deref())
        .collect();
    if condition_jobs.is_empty() {
        return false;
    }
    actions
        .iter()
        .filter_map(|action| action.job_name.as_deref())
        .any(|action_job| condition_jobs.contains(action_job))
}

/// The first workflow, job or crawler a trigger's definition names that this store holds
/// no record of, or `None` when every one of them is a stored entity.
///
/// A trigger belongs to the workflow it names and runs the actions and waits on the
/// conditions it carries, so the definition is resolvable only if those entities exist;
/// `region` is the Region the request is served in, which is where its job definitions
/// live, while a workflow and a crawler live in one per-account namespace. An
/// `Action$SecurityConfiguration` is NOT resolved here: it configures how an action runs
/// rather than naming the entity the action initiates.
///
/// The walk visits each action once and each condition once — the conditions bounded by
/// the modelled `ConditionList` length of at most 500, the actions by the request body the
/// transport accepted — and every lookup is a map probe.
pub fn missing_trigger_reference<'a>(
    store: &ComAmazonawsGlueAwsglueState,
    region: &str,
    workflow_name: Option<&'a str>,
    actions: &'a [Action],
    predicate: Option<&'a Predicate>,
) -> Option<TriggerReference<'a>> {
    if let Some(workflow_name) = workflow_name {
        if store.workflow(workflow_name).is_none() {
            return Some(TriggerReference::Workflow(workflow_name));
        }
    }
    let action_targets = actions
        .iter()
        .map(|action| (action.job_name.as_deref(), action.crawler_name.as_deref()));
    let condition_targets = predicate
        .and_then(|predicate| predicate.conditions.as_ref())
        .into_iter()
        .flatten()
        .map(|condition| {
            (
                condition.job_name.as_deref(),
                condition.crawler_name.as_deref(),
            )
        });
    for (job_name, crawler_name) in action_targets.chain(condition_targets) {
        if let Some(job_name) = job_name {
            if store.job(region, job_name).is_none() {
                return Some(TriggerReference::Job(job_name));
            }
        }
        if let Some(crawler_name) = crawler_name {
            if store.crawler(crawler_name).is_none() {
                return Some(TriggerReference::Crawler(crawler_name));
            }
        }
    }
    None
}
