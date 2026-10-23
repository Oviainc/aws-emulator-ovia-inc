// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-swf-simpleworkflowservice service.
//!
//! One map of domains; a domain owns its registered workflow/activity types and its
//! workflow executions, and an execution owns its history event log. Every handler
//! computes its response from what is stored here — nothing is canned and nothing is
//! echoed back out of the request that asked for it.

use std::collections::{HashMap, HashSet};

use ::aws_smithy_types::DateTime;
use ::sha2::{Digest, Sha256};

use crate::ops::types::{
    ActivityType, ActivityTypeConfiguration, ActivityTypeInfo,
    DecisionTaskScheduledEventAttributes, DomainConfiguration, DomainInfo, ExecutionTimeFilter,
    HistoryEvent, ResourceTag, TagFilter, TaskList, WorkflowExecution,
    WorkflowExecutionCancelRequestedEventAttributes,
    WorkflowExecutionConfiguration, WorkflowExecutionFilter, WorkflowExecutionInfo,
    WorkflowExecutionOpenCounts, WorkflowExecutionStartedEventAttributes,
    WorkflowExecutionTerminatedEventAttributes, WorkflowType, WorkflowTypeConfiguration,
    WorkflowTypeFilter, WorkflowTypeInfo,
};

/// The region an ARN carries when the request is unsigned (so it has no SigV4
/// credential scope to read a region from). `emulator_arn`'s own documented default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace (and sigv4 signing name) of this service, as the model's
/// `aws.api#service` trait records it (`arnNamespace: swf`).
pub const ARN_NAMESPACE: &str = "swf";

/// `RegistrationStatus` of a freshly registered domain or type. One of
/// [`crate::ops::types::REGISTRATION_STATUS_VALUES`].
pub const STATUS_REGISTERED: &str = "REGISTERED";

/// `RegistrationStatus` of a domain or type that has been deprecated. One of
/// [`crate::ops::types::REGISTRATION_STATUS_VALUES`]; `RegistrationStatus`'s own doc
/// calls it "DEPRECATED – The type was deprecated".
pub const STATUS_DEPRECATED: &str = "DEPRECATED";

/// `ExecutionStatus` of a running execution. One of
/// [`crate::ops::types::EXECUTION_STATUS_VALUES`].
pub const EXECUTION_STATUS_OPEN: &str = "OPEN";

/// `ExecutionStatus` of an execution that has closed. One of
/// [`crate::ops::types::EXECUTION_STATUS_VALUES`].
pub const EXECUTION_STATUS_CLOSED: &str = "CLOSED";

/// `CloseStatus` of an execution closed by `TerminateWorkflowExecution`. One of
/// [`crate::ops::types::CLOSE_STATUS_VALUES`].
pub const CLOSE_STATUS_TERMINATED: &str = "TERMINATED";

/// The maximum `workflowExecutionRetentionPeriodInDays` RegisterDomain accepts. The
/// member's own doc: "The maximum workflow execution retention period is 90 days."
pub const MAX_RETENTION_PERIOD_IN_DAYS: u32 = 90;

/// The largest value an execution start-to-close timeout may take, in seconds. The
/// `executionStartToCloseTimeout` doc: "there is a one-year max limit on the time that
/// a workflow execution can run."
pub const MAX_EXECUTION_START_TO_CLOSE_SECONDS: u64 = 365 * 24 * 60 * 60;

/// Tags one domain may carry; `RegisterDomain`'s modelled `TooManyTagsFault` ("You've
/// exceeded the number of tags allowed for a domain") is the answer past it. Amazon SWF
/// documents a limit of 50 tags per domain.
pub const MAX_TAGS_PER_DOMAIN: usize = 50;

/// How many domains this emulator holds. Emulator-chosen, so a caller cannot grow the
/// in-memory store without limit; reported with the modelled `LimitExceededFault`
/// ("Returned by any operation if a system imposed limitation has been reached").
pub const MAX_DOMAINS: usize = 100;

/// How many workflow types one domain holds. Emulator-chosen, for the same reason as
/// [`MAX_DOMAINS`], and answered with the same modelled `LimitExceededFault`.
pub const MAX_WORKFLOW_TYPES_PER_DOMAIN: usize = 1_000;

/// How many activity types one domain holds. Emulator-chosen, for the same reason as
/// [`MAX_DOMAINS`], and answered with the same modelled `LimitExceededFault`.
pub const MAX_ACTIVITY_TYPES_PER_DOMAIN: usize = 1_000;

/// How many workflow executions one domain retains. Emulator-chosen, for the same
/// reason as [`MAX_DOMAINS`], and answered with the same modelled
/// `LimitExceededFault`.
pub const MAX_EXECUTIONS_PER_DOMAIN: usize = 1_000;

/// The largest count a visibility `Count*` operation reports in one answer. The
/// modelled `truncated` member is what reports a count clipped to it: "If set to true,
/// indicates that the actual count was more than the maximum supported by this API and
/// the count returned is the truncated value." Emulator-chosen, and equal to
/// [`MAX_EXECUTIONS_PER_DOMAIN`] — the most executions one domain can hold — so a count
/// this emulator can actually reach is never clipped and `truncated` stays false.
pub const MAX_REPORTED_EXECUTION_COUNT: usize = MAX_EXECUTIONS_PER_DOMAIN;

/// The page size `GetWorkflowExecutionHistory` uses when the caller passes no
/// `maximumPageSize` (the member defaults to `0`), so one unparameterised request
/// cannot materialise a response that grows without bound. It is the modelled
/// `PageSize` maximum (range 0..1000).
pub const MAX_HISTORY_PAGE_SIZE: usize = 1_000;

/// The page size a visibility list of workflow executions uses when the caller passes
/// no `maximumPageSize` (the member defaults to `0`), so one unparameterised request
/// cannot materialise a response that grows without bound. It is the modelled
/// `PageSize` maximum (range 0..1000).
pub const MAX_EXECUTION_PAGE_SIZE: usize = 1_000;

/// A registered workflow type, as `RegisterWorkflowType` left it. The `default_*`
/// members are what `StartWorkflowExecution` falls back to when the start request
/// does not override them.
#[derive(Debug, Clone)]
pub struct WorkflowTypeRecord {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    /// One of [`crate::ops::types::REGISTRATION_STATUS_VALUES`].
    pub status: String,
    pub creation_date: DateTime,
    pub deprecation_date: Option<DateTime>,
    pub default_task_list: Option<String>,
    pub default_task_start_to_close_timeout: Option<String>,
    pub default_execution_start_to_close_timeout: Option<String>,
    pub default_task_priority: Option<String>,
    pub default_child_policy: Option<String>,
    pub default_lambda_role: Option<String>,
}

impl WorkflowTypeRecord {
    /// The `WorkflowType` identity this record is keyed by.
    pub fn workflow_type(&self) -> WorkflowType {
        WorkflowType {
            name: self.name.clone(),
            version: self.version.clone(),
        }
    }

    /// Whether this type is deprecated, i.e. whether its stored `RegistrationStatus`
    /// is [`STATUS_DEPRECATED`].
    pub fn is_deprecated(&self) -> bool {
        self.status == STATUS_DEPRECATED
    }

    /// Move this type off `REGISTERED` as of `at`: the one-way status move every
    /// deprecation of a workflow type makes, whether `DeprecateWorkflowType` addressed
    /// this record directly or [`Domain::deprecate`] cascaded to it. A record that is
    /// already deprecated keeps the `deprecationDate` its own deprecation gave it, so
    /// no later call can rewrite an instant that already happened.
    pub fn deprecate(&mut self, at: DateTime) {
        if self.is_deprecated() {
            return;
        }
        self.status = STATUS_DEPRECATED.to_string();
        self.deprecation_date = Some(at);
    }

    /// The `WorkflowTypeInfo` every workflow-type reader returns, built from what is
    /// stored.
    pub fn info(&self) -> WorkflowTypeInfo {
        WorkflowTypeInfo {
            workflow_type: self.workflow_type(),
            status: self.status.clone(),
            description: self.description.clone(),
            creation_date: self.creation_date,
            deprecation_date: self.deprecation_date,
        }
    }

    /// The `WorkflowTypeConfiguration` `DescribeWorkflowType` reports: the
    /// registration-time defaults, exactly as stored.
    pub fn configuration(&self) -> WorkflowTypeConfiguration {
        WorkflowTypeConfiguration {
            default_task_start_to_close_timeout: self.default_task_start_to_close_timeout.clone(),
            default_execution_start_to_close_timeout: self
                .default_execution_start_to_close_timeout
                .clone(),
            default_task_list: self
                .default_task_list
                .clone()
                .map(|name| TaskList { name }),
            default_task_priority: self.default_task_priority.clone(),
            default_child_policy: self.default_child_policy.clone(),
            default_lambda_role: self.default_lambda_role.clone(),
        }
    }
}

/// A registered activity type, as `RegisterActivityType` left it. The `default_*`
/// members are the registration-time settings a `ScheduleActivityTask` decision falls
/// back to when it does not override them, and are reported verbatim by
/// `DescribeActivityType`: the type's configuration cannot change after registration.
#[derive(Debug, Clone)]
pub struct ActivityTypeRecord {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    /// One of [`crate::ops::types::REGISTRATION_STATUS_VALUES`].
    pub status: String,
    pub creation_date: DateTime,
    pub deprecation_date: Option<DateTime>,
    pub default_task_list: Option<String>,
    pub default_task_priority: Option<String>,
    pub default_task_heartbeat_timeout: Option<String>,
    pub default_task_schedule_to_close_timeout: Option<String>,
    pub default_task_schedule_to_start_timeout: Option<String>,
    pub default_task_start_to_close_timeout: Option<String>,
}

impl ActivityTypeRecord {
    /// The `ActivityType` identity this record is keyed by.
    pub fn activity_type(&self) -> ActivityType {
        ActivityType {
            name: self.name.clone(),
            version: self.version.clone(),
        }
    }

    /// The `ActivityTypeInfo` every activity-type reader returns, built from what is
    /// stored.
    pub fn info(&self) -> ActivityTypeInfo {
        ActivityTypeInfo {
            activity_type: self.activity_type(),
            status: self.status.clone(),
            description: self.description.clone(),
            creation_date: self.creation_date,
            deprecation_date: self.deprecation_date,
        }
    }

    /// The `ActivityTypeConfiguration` `DescribeActivityType` reports: the
    /// registration-time defaults, exactly as stored.
    pub fn configuration(&self) -> ActivityTypeConfiguration {
        ActivityTypeConfiguration {
            default_task_start_to_close_timeout: self.default_task_start_to_close_timeout.clone(),
            default_task_heartbeat_timeout: self.default_task_heartbeat_timeout.clone(),
            default_task_list: self
                .default_task_list
                .clone()
                .map(|name| TaskList { name }),
            default_task_priority: self.default_task_priority.clone(),
            default_task_schedule_to_start_timeout: self
                .default_task_schedule_to_start_timeout
                .clone(),
            default_task_schedule_to_close_timeout: self
                .default_task_schedule_to_close_timeout
                .clone(),
        }
    }
}

/// The timeouts, task list and child policy an execution actually runs under:
/// resolved ONCE at start (the request value where given, else the workflow type's
/// registered default) and reported verbatim by `DescribeWorkflowExecution`, so a
/// later re-registration of the type cannot change what a running execution reports.
#[derive(Debug, Clone)]
pub struct ExecutionConfig {
    pub task_list: String,
    pub task_priority: Option<String>,
    pub task_start_to_close_timeout: String,
    pub execution_start_to_close_timeout: String,
    /// One of [`crate::ops::types::CHILD_POLICY_VALUES`].
    pub child_policy: String,
    pub lambda_role: Option<String>,
}

impl ExecutionConfig {
    /// The `WorkflowExecutionConfiguration` `DescribeWorkflowExecution` reports.
    pub fn configuration(&self) -> WorkflowExecutionConfiguration {
        WorkflowExecutionConfiguration {
            task_start_to_close_timeout: self.task_start_to_close_timeout.clone(),
            execution_start_to_close_timeout: self.execution_start_to_close_timeout.clone(),
            task_list: TaskList {
                name: self.task_list.clone(),
            },
            task_priority: self.task_priority.clone(),
            child_policy: self.child_policy.clone(),
            lambda_role: self.lambda_role.clone(),
        }
    }
}

/// The attributes of one recorded history event. One variant per event type this slice
/// records; the variant IS the event type, so a stored event can never carry the
/// attributes of a different kind than its `eventType` claims.
#[derive(Debug, Clone)]
pub enum EventAttributes {
    /// Boxed: `WorkflowExecutionStartedEventAttributes` is several times the size of
    /// the other variants, and every stored history event pays for the largest one.
    WorkflowExecutionStarted(Box<WorkflowExecutionStartedEventAttributes>),
    DecisionTaskScheduled(DecisionTaskScheduledEventAttributes),
    WorkflowExecutionTerminated(WorkflowExecutionTerminatedEventAttributes),
    WorkflowExecutionCancelRequested(WorkflowExecutionCancelRequestedEventAttributes),
}

impl EventAttributes {
    /// The `eventType` these attributes belong to — one of
    /// [`crate::ops::types::EVENT_TYPE_VALUES`].
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::WorkflowExecutionStarted(_) => "WorkflowExecutionStarted",
            Self::DecisionTaskScheduled(_) => "DecisionTaskScheduled",
            Self::WorkflowExecutionTerminated(_) => "WorkflowExecutionTerminated",
            Self::WorkflowExecutionCancelRequested(_) => "WorkflowExecutionCancelRequested",
        }
    }

    /// The `scheduledEventId` of the decision task these attributes take OUT of its
    /// task list, when that is what the event records.
    ///
    /// A scheduled decision task waits in its task list until a decider is handed it
    /// (`DecisionTaskStarted`, whose `scheduledEventId` names the task it started) or
    /// it times out there first (`DecisionTaskTimedOut`, same member). No stored
    /// variant records either event yet — the operations that would write them
    /// (`PollForDecisionTask` and the start-to-close sweep behind it) are still
    /// scaffolds tracked by their own gap issues — so every event answers `None`
    /// today. This exhaustive match is the single place that enumerates them, so a
    /// variant added later cannot be forgotten by
    /// [`Execution::pending_decision_tasks_on`].
    pub fn decision_task_left_task_list(&self) -> Option<i64> {
        match self {
            Self::WorkflowExecutionStarted(_)
            | Self::DecisionTaskScheduled(_)
            | Self::WorkflowExecutionTerminated(_)
            | Self::WorkflowExecutionCancelRequested(_) => None,
        }
    }
}

/// One event of a run's persisted history.
#[derive(Debug, Clone)]
pub struct HistoryEventRecord {
    pub event_id: i64,
    pub event_timestamp: DateTime,
    pub attributes: EventAttributes,
}

impl HistoryEventRecord {
    /// The modelled `HistoryEvent` for this record: the `eventType` the variant names,
    /// and the one matching `*EventAttributes` member populated. Every other
    /// attributes member stays absent — an event carries exactly one.
    pub fn event(&self) -> HistoryEvent {
        let mut event = blank_history_event(
            self.event_id,
            self.event_timestamp,
            self.attributes.event_type(),
        );
        match &self.attributes {
            EventAttributes::WorkflowExecutionStarted(attributes) => {
                event.workflow_execution_started_event_attributes =
                    Some(attributes.as_ref().clone());
            }
            EventAttributes::DecisionTaskScheduled(attributes) => {
                event.decision_task_scheduled_event_attributes = Some(attributes.clone());
            }
            EventAttributes::WorkflowExecutionTerminated(attributes) => {
                event.workflow_execution_terminated_event_attributes = Some(attributes.clone());
            }
            EventAttributes::WorkflowExecutionCancelRequested(attributes) => {
                event.workflow_execution_cancel_requested_event_attributes =
                    Some(attributes.clone());
            }
        }
        event
    }
}

/// A `HistoryEvent` with its identity set and EVERY `*EventAttributes` member absent.
/// `HistoryEvent` models no default (its `eventTimestamp`/`eventType`/`eventId` are
/// required), and it carries one optional member per event type, so this is the single
/// place that enumerates them — [`HistoryEventRecord::event`] then fills in the one
/// that matches.
fn blank_history_event(event_id: i64, event_timestamp: DateTime, event_type: &str) -> HistoryEvent {
    HistoryEvent {
        event_timestamp,
        event_type: event_type.to_string(),
        event_id,
        workflow_execution_started_event_attributes: None,
        workflow_execution_completed_event_attributes: None,
        complete_workflow_execution_failed_event_attributes: None,
        workflow_execution_failed_event_attributes: None,
        fail_workflow_execution_failed_event_attributes: None,
        workflow_execution_timed_out_event_attributes: None,
        workflow_execution_canceled_event_attributes: None,
        cancel_workflow_execution_failed_event_attributes: None,
        workflow_execution_continued_as_new_event_attributes: None,
        continue_as_new_workflow_execution_failed_event_attributes: None,
        workflow_execution_terminated_event_attributes: None,
        workflow_execution_cancel_requested_event_attributes: None,
        decision_task_scheduled_event_attributes: None,
        decision_task_started_event_attributes: None,
        decision_task_completed_event_attributes: None,
        decision_task_timed_out_event_attributes: None,
        activity_task_scheduled_event_attributes: None,
        activity_task_started_event_attributes: None,
        activity_task_completed_event_attributes: None,
        activity_task_failed_event_attributes: None,
        activity_task_timed_out_event_attributes: None,
        activity_task_canceled_event_attributes: None,
        activity_task_cancel_requested_event_attributes: None,
        workflow_execution_signaled_event_attributes: None,
        marker_recorded_event_attributes: None,
        record_marker_failed_event_attributes: None,
        timer_started_event_attributes: None,
        timer_fired_event_attributes: None,
        timer_canceled_event_attributes: None,
        start_child_workflow_execution_initiated_event_attributes: None,
        child_workflow_execution_started_event_attributes: None,
        child_workflow_execution_completed_event_attributes: None,
        child_workflow_execution_failed_event_attributes: None,
        child_workflow_execution_timed_out_event_attributes: None,
        child_workflow_execution_canceled_event_attributes: None,
        child_workflow_execution_terminated_event_attributes: None,
        signal_external_workflow_execution_initiated_event_attributes: None,
        external_workflow_execution_signaled_event_attributes: None,
        signal_external_workflow_execution_failed_event_attributes: None,
        external_workflow_execution_cancel_requested_event_attributes: None,
        request_cancel_external_workflow_execution_initiated_event_attributes: None,
        request_cancel_external_workflow_execution_failed_event_attributes: None,
        schedule_activity_task_failed_event_attributes: None,
        request_cancel_activity_task_failed_event_attributes: None,
        start_timer_failed_event_attributes: None,
        cancel_timer_failed_event_attributes: None,
        start_child_workflow_execution_failed_event_attributes: None,
        lambda_function_scheduled_event_attributes: None,
        lambda_function_started_event_attributes: None,
        lambda_function_completed_event_attributes: None,
        lambda_function_failed_event_attributes: None,
        lambda_function_timed_out_event_attributes: None,
        schedule_lambda_function_failed_event_attributes: None,
        start_lambda_function_failed_event_attributes: None,
    }
}

/// One workflow execution — a `(workflowId, runId)` pair — and its history.
#[derive(Debug, Clone)]
pub struct Execution {
    pub workflow_id: String,
    pub run_id: String,
    /// The `(name, version)` of the workflow type this run was started with.
    pub workflow_type: (String, String),
    /// One of [`crate::ops::types::EXECUTION_STATUS_VALUES`].
    pub status: String,
    /// One of [`crate::ops::types::CLOSE_STATUS_VALUES`], set only once closed.
    pub close_status: Option<String>,
    pub start_timestamp: DateTime,
    pub close_timestamp: Option<DateTime>,
    pub tag_list: Vec<String>,
    pub cancel_requested: bool,
    pub input: Option<String>,
    pub effective_config: ExecutionConfig,
    /// The persisted event log, oldest first. Append only through
    /// [`Execution::append_event`], which is the single `event_id` allocator.
    pub history: Vec<HistoryEventRecord>,
    /// The id the NEXT appended event takes; ids therefore stay unique and strictly
    /// increasing within a run. The first event of a run is `1`.
    pub next_event_id: i64,
    pub open_decision_tasks: i32,
    pub open_activity_tasks: i32,
    pub open_timers: i32,
    pub open_child_executions: i32,
    pub open_lambda_functions: i32,
    pub latest_execution_context: Option<String>,
    /// When an activity task of this run was last scheduled/started — what
    /// `DescribeWorkflowExecution`'s `latestActivityTaskTimestamp` reports. Written by
    /// the activity-task operations, which are tracked as their own gap issues.
    pub latest_activity_task_timestamp: Option<DateTime>,
}

impl Execution {
    /// Append one event to this run's history and return the id it was given. The ONLY
    /// writer of `history`/`next_event_id`, so no two events of a run share an id.
    pub fn append_event(&mut self, at: DateTime, attributes: EventAttributes) -> i64 {
        let event_id = self.next_event_id;
        self.next_event_id += 1;
        self.history.push(HistoryEventRecord {
            event_id,
            event_timestamp: at,
            attributes,
        });
        event_id
    }

    /// The `WorkflowExecution` identity this run is keyed by.
    pub fn execution(&self) -> WorkflowExecution {
        WorkflowExecution {
            workflow_id: self.workflow_id.clone(),
            run_id: self.run_id.clone(),
        }
    }

    /// The `WorkflowExecutionInfo` every execution reader returns, built from what is
    /// stored.
    pub fn info(&self) -> WorkflowExecutionInfo {
        WorkflowExecutionInfo {
            execution: self.execution(),
            workflow_type: WorkflowType {
                name: self.workflow_type.0.clone(),
                version: self.workflow_type.1.clone(),
            },
            start_timestamp: self.start_timestamp,
            close_timestamp: self.close_timestamp,
            execution_status: self.status.clone(),
            close_status: self.close_status.clone(),
            // Child executions are not startable in this slice (the decision
            // operations are still 501), so no stored run has a parent to report.
            parent: None,
            tag_list: (!self.tag_list.is_empty()).then(|| self.tag_list.clone()),
            cancel_requested: self.cancel_requested,
        }
    }

    /// The `WorkflowExecutionOpenCounts` `DescribeWorkflowExecution` reports, counted
    /// from this run's live state.
    pub fn open_counts(&self) -> WorkflowExecutionOpenCounts {
        WorkflowExecutionOpenCounts {
            open_activity_tasks: self.open_activity_tasks,
            open_decision_tasks: self.open_decision_tasks,
            open_timers: self.open_timers,
            open_child_workflow_executions: self.open_child_executions,
            open_lambda_functions: self.open_lambda_functions,
        }
    }

    /// Whether this run is still OPEN.
    pub fn is_open(&self) -> bool {
        self.status == EXECUTION_STATUS_OPEN
    }

    /// How many of this run's decision tasks are still WAITING in `task_list`: one per
    /// `DecisionTaskScheduled` event of its history that names that list and that no
    /// later event has taken out of it (see
    /// [`EventAttributes::decision_task_left_task_list`]).
    ///
    /// The history IS the record of which tasks were scheduled where — the scheduling
    /// event carries its own `taskList`, so a run whose decision tasks were scheduled
    /// on more than one list contributes to each list separately.
    pub fn pending_decision_tasks_on(&self, task_list: &str) -> usize {
        let taken: HashSet<i64> = self
            .history
            .iter()
            .filter_map(|event| event.attributes.decision_task_left_task_list())
            .collect();
        self.history
            .iter()
            .filter(|event| match &event.attributes {
                EventAttributes::DecisionTaskScheduled(scheduled) => {
                    scheduled.task_list.name == task_list && !taken.contains(&event.event_id)
                }
                _ => false,
            })
            .count()
    }
}

/// One registered domain and everything scoped to it.
#[derive(Debug, Clone)]
pub struct Domain {
    pub name: String,
    pub description: Option<String>,
    pub retention_period_in_days: String,
    /// One of [`crate::ops::types::REGISTRATION_STATUS_VALUES`].
    pub status: String,
    pub arn: String,
    pub tags: Vec<ResourceTag>,
    /// `(name, version)` -> the registered workflow type.
    pub workflow_types: HashMap<(String, String), WorkflowTypeRecord>,
    /// `(name, version)` -> the registered activity type, written by
    /// `RegisterActivityType` and read back by `DescribeActivityType`.
    pub activity_types: HashMap<(String, String), ActivityTypeRecord>,
    /// `(workflowId, runId)` -> the execution.
    pub executions: HashMap<(String, String), Execution>,
    /// `workflowId` -> the `runId` of the one OPEN run with that id. This is what
    /// makes a second start of a still-open `workflowId` a
    /// `WorkflowExecutionAlreadyStartedFault`, and what resolves a
    /// Terminate/Signal/Cancel call that omits `runId`.
    pub open_by_workflow_id: HashMap<String, String>,
    /// The ordinal the NEXT start in this domain takes, feeding
    /// [`new_run_id`]. Strictly increasing and never reused, so no two runs of this
    /// domain can be given the same `runId`.
    pub next_run_sequence: u64,
}

impl Domain {
    /// A domain registered now under `name`, with no types and no executions.
    pub fn registered(name: &str, region: &str, retention_period_in_days: &str) -> Self {
        Self {
            name: name.to_string(),
            description: None,
            retention_period_in_days: retention_period_in_days.to_string(),
            status: STATUS_REGISTERED.to_string(),
            arn: domain_arn(region, name),
            tags: Vec::new(),
            workflow_types: HashMap::new(),
            activity_types: HashMap::new(),
            executions: HashMap::new(),
            open_by_workflow_id: HashMap::new(),
            next_run_sequence: 0,
        }
    }

    /// The `DomainInfo` every domain reader returns, built from what is stored.
    pub fn info(&self) -> DomainInfo {
        DomainInfo {
            name: self.name.clone(),
            status: self.status.clone(),
            description: self.description.clone(),
            arn: Some(self.arn.clone()),
        }
    }

    /// The `DomainConfiguration` `DescribeDomain` reports: the retention period this
    /// domain was registered with, exactly as stored.
    pub fn configuration(&self) -> DomainConfiguration {
        DomainConfiguration {
            workflow_execution_retention_period_in_days: self.retention_period_in_days.clone(),
        }
    }

    /// Whether this domain is deprecated, i.e. whether its stored
    /// `RegistrationStatus` is [`STATUS_DEPRECATED`].
    pub fn is_deprecated(&self) -> bool {
        self.status == STATUS_DEPRECATED
    }

    /// Deprecate this domain as of `at`, cascading to every type registered in it.
    /// `DeprecateDomain`'s doc: "Deprecating a domain also deprecates all activity and
    /// workflow types registered in the domain." A type that was already deprecated
    /// keeps the `deprecationDate` its own deprecation gave it, so the cascade cannot
    /// rewrite an instant that already happened.
    pub fn deprecate(&mut self, at: DateTime) {
        self.status = STATUS_DEPRECATED.to_string();
        for workflow_type in self.workflow_types.values_mut() {
            workflow_type.deprecate(at);
        }
        for activity_type in self.activity_types.values_mut() {
            if activity_type.status != STATUS_DEPRECATED {
                activity_type.status = STATUS_DEPRECATED.to_string();
                activity_type.deprecation_date = Some(at);
            }
        }
    }

    /// How many decision tasks are waiting in `task_list` across this domain — what
    /// `CountPendingDecisionTasks` answers with. A task list is scoped to its domain,
    /// so only this domain's runs contribute, and a list no decision task was ever
    /// scheduled in holds none.
    ///
    /// Only an OPEN run has tasks in the list: closing a run drops whatever decision
    /// tasks it had waiting, which is the same reason
    /// `DescribeWorkflowExecution` reports no open decision task for a closed run.
    pub fn pending_decision_tasks_on(&self, task_list: &str) -> usize {
        self.executions
            .values()
            .filter(|execution| execution.is_open())
            .map(|execution| execution.pending_decision_tasks_on(task_list))
            .sum()
    }

    /// The `runId` a call naming `workflow_id` addresses: the one it gave, when a run
    /// with that pair is stored, else the currently OPEN run of that `workflowId`.
    /// `None` when neither resolves to a stored execution.
    pub fn resolve_run_id(&self, workflow_id: &str, run_id: Option<&str>) -> Option<String> {
        match run_id.filter(|id| !id.is_empty()) {
            Some(id) => self
                .executions
                .contains_key(&(workflow_id.to_string(), id.to_string()))
                .then(|| id.to_string()),
            None => self.open_by_workflow_id.get(workflow_id).cloned(),
        }
    }
}

/// Per-service state for com-amazonaws-swf-simpleworkflowservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSwfSimpleworkflowserviceState {
    /// Domain name -> the domain. Domain names are unique per region, and this
    /// emulator serves one account, so the name is the whole key.
    pub domains: HashMap<String, Domain>,
}

impl ComAmazonawsSwfSimpleworkflowserviceState {
    /// The domain a `resourceArn` addresses: the stored domain whose own ARN is
    /// exactly this one.
    ///
    /// Comparing the WHOLE ARN, not just the name parsed out of it, is what scopes the
    /// lookup to the resource's region and account — `UnknownResourceFault` is
    /// documented as "Returned when the named resource cannot be found with in the
    /// scope of this operation (region or domain)" — so one region's ARN never
    /// resolves to a same-named domain registered in another.
    pub fn domain_by_arn(&self, arn: &str) -> Option<&Domain> {
        let name = domain_name_from_arn(arn)?;
        self.domains.get(name).filter(|domain| domain.arn == arn)
    }
}

/// The resource portion of a domain ARN, before the domain's own name. SWF's domain
/// ARNs are the slash-prefixed form the service's own `domainInfo.arn` carries.
pub const DOMAIN_RESOURCE_PREFIX: &str = "/domain/";

/// The ARN of a domain: `arn:aws:swf:<region>:<account>:/domain/<name>`.
pub fn domain_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{DOMAIN_RESOURCE_PREFIX}{name}"),
    )
}

/// The domain name a `resourceArn` addresses, or `None` when the string is not a
/// domain ARN of this service.
///
/// The inverse of [`domain_arn`]: the tagging operations take an ARN where every other
/// operation takes a domain name, so they resolve the name back out of it. A domain
/// name can contain neither `:` nor `/` ([`validate_name`]), so the segment after the
/// resource prefix is the whole name.
pub fn domain_name_from_arn(arn: &str) -> Option<&str> {
    let mut segments = arn.splitn(6, ':');
    let prefix = segments.next()?;
    let _partition = segments.next()?;
    let service = segments.next()?;
    let _region = segments.next()?;
    let _account = segments.next()?;
    let resource = segments.next()?;
    if prefix != "arn" || service != ARN_NAMESPACE {
        return None;
    }
    let name = resource.strip_prefix(DOMAIN_RESOURCE_PREFIX)?;
    (!name.is_empty()).then_some(name)
}

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`] when the
/// request carries no SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Now, as the wire timestamp type every modelled timestamp member uses.
pub fn now() -> DateTime {
    DateTime::from(::std::time::SystemTime::now())
}

/// A freshly minted, unique `runId` for a start in `domain`.
///
/// `sequence` comes from [`Domain::next_run_sequence`], which is strictly increasing
/// per domain, so `(domain, sequence)` is unique across the store and therefore so is
/// this id — two starts in the same nanosecond still get different ones. The SHA-256
/// hex digest is 64 characters, exactly the modelled `WorkflowRunId` maximum, and
/// makes the id opaque the way the real service's run ids are: a caller cannot derive
/// it from its own request.
pub fn new_run_id(domain: &str, workflow_id: &str, sequence: u64, at: DateTime) -> String {
    let mut digest = Sha256::new();
    digest.update(domain.as_bytes());
    digest.update([0u8]);
    digest.update(workflow_id.as_bytes());
    digest.update([0u8]);
    digest.update(sequence.to_be_bytes());
    digest.update(at.secs().to_be_bytes());
    digest.update(at.subsec_nanos().to_be_bytes());
    ::hex::encode(digest.finalize())
}

/// An optional string member, with an empty string read as "no value given".
///
/// The model draws exactly this distinction: every overridable duration targets
/// `DurationInSecondsOptional` (length 0..8, so `""` is on the wire) while every
/// duration that must carry a value targets `DurationInSeconds` (length 1..8). So an
/// empty optional member is an absent one, and falls back to the registered default.
pub fn non_empty(value: &Option<String>) -> Option<String> {
    value.as_ref().filter(|text| !text.is_empty()).cloned()
}

/// `GetWorkflowExecutionHistory`'s `nextPageToken` for the next unread event: the
/// ordering the page was read in, then the `eventId` the page ended on.
///
/// The token names an EVENT, not a position, so appending to the log between pages
/// cannot shift what it addresses — which matters most under `reverseOrder`, where a
/// newly appended event lands at the front of the ordering. The ordering is part of
/// the token because "the event after this one" differs between the two orderings;
/// carrying it lets a token replayed against the other ordering be refused instead of
/// silently skipping or repeating events.
pub fn encode_history_page_token(reverse_order: bool, last_event_id: i64) -> String {
    format!("{}{last_event_id}", history_order_tag(reverse_order))
}

/// The `eventId` a `nextPageToken` resumes AFTER, for a request reading in
/// `reverse_order`.
pub fn decode_history_page_token(token: &str, reverse_order: bool) -> Result<i64, String> {
    let expected = history_order_tag(reverse_order);
    let event_id = token
        .strip_prefix(expected)
        .and_then(|event_id| event_id.parse::<i64>().ok());
    event_id.ok_or_else(|| {
        format!(
            "nextPageToken {token:?} was not issued for a reverseOrder={reverse_order} read of \
             this execution's history; pass the token back with the same reverseOrder"
        )
    })
}

/// A timestamp as an orderable pair of whole seconds and nanoseconds.
///
/// The wire timestamp type carries no total order of its own, and every visibility
/// listing has to order executions by time, so this is the single place that turns one
/// into a comparable key.
pub fn instant_key(at: DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// The total order a visibility listing of workflow executions pages through: the
/// execution's start instant, then its `(workflowId, runId)` identity.
///
/// The identity is part of the key because the store holds executions in a map with no
/// order of its own: two runs started in the same instant would otherwise have no
/// defined position, and a page boundary between them would not be reproducible.
pub type ExecutionOrderKey = ((i64, u32), String, String);

/// The [`ExecutionOrderKey`] of one stored execution.
pub fn execution_order_key(execution: &Execution) -> ExecutionOrderKey {
    (
        instant_key(execution.start_timestamp),
        execution.workflow_id.clone(),
        execution.run_id.clone(),
    )
}

/// Whether an execution's start (or close) instant falls inside an
/// `ExecutionTimeFilter` window.
///
/// The filter's doc: "Each parameter, if specified, defines a rule that must be
/// satisfied by each returned query result" — `oldestDate` is required and
/// `latestDate` is optional, so an absent `latestDate` leaves the window open-ended at
/// the recent end. Both ends are inclusive.
pub fn within_time_window(at: DateTime, filter: &ExecutionTimeFilter) -> bool {
    let at = instant_key(at);
    at >= instant_key(filter.oldest_date)
        && filter
            .latest_date
            .is_none_or(|latest| at <= instant_key(latest))
}

/// Whether an execution satisfies the `executionFilter` / `typeFilter` / `tagFilter`
/// the request named. A filter that was not given constrains nothing.
///
/// Each member's doc states its own rule: `executionFilter` keeps "only workflow
/// executions matching the workflow ID specified in the filter", `typeFilter` "only
/// executions of the type specified in the filter" (whose `version` is optional, so an
/// absent one keeps every version registered under that name), and `tagFilter` "only
/// executions that have the matching tag".
pub fn execution_matches_filters(
    execution: &Execution,
    execution_filter: Option<&WorkflowExecutionFilter>,
    type_filter: Option<&WorkflowTypeFilter>,
    tag_filter: Option<&TagFilter>,
) -> bool {
    let id_matches =
        execution_filter.is_none_or(|filter| execution.workflow_id == filter.workflow_id);
    let type_matches = type_filter.is_none_or(|filter| {
        execution.workflow_type.0 == filter.name
            && non_empty(&filter.version).is_none_or(|version| execution.workflow_type.1 == version)
    });
    let tag_matches = tag_filter.is_none_or(|filter| execution.tag_list.contains(&filter.tag));
    id_matches && type_matches && tag_matches
}

/// Refuse a visibility request that names more than one of the mutually exclusive
/// filters, listing the ones it named.
///
/// Every visibility operation documents the same rule on each of them:
/// "executionFilter, typeFilter and tagFilter are mutually exclusive. You can specify
/// at most one of these in a request." The model cannot express it, so the boundary
/// validator passes it and the handler applies this.
pub fn validate_exclusive_filters(filters: &[(&str, bool)]) -> Result<(), String> {
    let named: Vec<&str> = filters
        .iter()
        .filter(|(_, given)| *given)
        .map(|(member, _)| *member)
        .collect();
    if named.len() > 1 {
        return Err(format!(
            "{} are mutually exclusive; specify at most one of them in a request, got {}",
            filters
                .iter()
                .map(|(member, _)| *member)
                .collect::<Vec<_>>()
                .join(", "),
            named.join(" and ")
        ));
    }
    Ok(())
}

/// A visibility listing's `nextPageToken` for the next unread execution: the ordering
/// the page was read in, then the [`ExecutionOrderKey`] the page ended on.
///
/// The token names a POSITION IN THE ORDER rather than a stored execution, so a run
/// that starts or closes between two pages cannot invalidate it: the next page is
/// whatever still sorts after that key. The ordering is part of the token because
/// "after this one" means the opposite thing in the two orderings; carrying it lets a
/// token replayed against the other ordering be refused instead of silently skipping
/// or repeating executions.
pub fn encode_execution_page_token(reverse_order: bool, key: &ExecutionOrderKey) -> String {
    let ((secs, nanos), workflow_id, run_id) = key;
    // Hex-encoded because the token is a response member with no structure of its own,
    // and a `workflowId` may hold any character SWF's naming rule allows.
    let payload = format!("{secs}.{nanos}\u{0}{workflow_id}\u{0}{run_id}");
    format!(
        "{}{}",
        execution_order_tag(reverse_order),
        ::hex::encode(payload)
    )
}

/// The [`ExecutionOrderKey`] a `nextPageToken` resumes AFTER, for a request listing in
/// `reverse_order`.
pub fn decode_execution_page_token(
    token: &str,
    reverse_order: bool,
) -> Result<ExecutionOrderKey, String> {
    let rejected = || {
        format!(
            "nextPageToken {token:?} was not issued for a reverseOrder={reverse_order} listing \
             of this domain's workflow executions; pass the token back unchanged, with every \
             other argument unchanged"
        )
    };
    let payload = token
        .strip_prefix(execution_order_tag(reverse_order))
        .and_then(|encoded| ::hex::decode(encoded).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(rejected)?;
    let mut parts = payload.split('\u{0}');
    let (Some(instant), Some(workflow_id), Some(run_id), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(rejected());
    };
    let (secs, nanos) = instant.split_once('.').ok_or_else(rejected)?;
    let secs = secs.parse::<i64>().map_err(|_| rejected())?;
    let nanos = nanos.parse::<u32>().map_err(|_| rejected())?;
    Ok((
        (secs, nanos),
        workflow_id.to_string(),
        run_id.to_string(),
    ))
}

/// The ordering tag [`encode_execution_page_token`] stamps into a page token.
fn execution_order_tag(reverse_order: bool) -> &'static str {
    if reverse_order {
        "a"
    } else {
        "d"
    }
}

/// The ordering tag [`encode_history_page_token`] stamps into a page token.
fn history_order_tag(reverse_order: bool) -> &'static str {
    if reverse_order {
        "r"
    } else {
        "f"
    }
}

/// Refuse a name SWF's own naming rule rejects, naming the member and what is wrong.
///
/// The rule is stated identically on every user-supplied SWF name (domain `name`,
/// workflow type `name`/`version`, `workflowId`, task list `name`): "The specified
/// string must not start or end with whitespace. It must not contain a `:` (colon),
/// `/` (slash), `|` (vertical bar), or any control characters (`\u0000-\u001f` |
/// `\u007f-\u009f`). Also, it must not be the literal string `arn`." The model cannot
/// express it, so the boundary validator passes it and each handler applies this.
pub fn validate_name(member: &str, value: &str) -> Result<(), String> {
    if value.trim() != value {
        return Err(format!(
            "{member} must not start or end with whitespace, got {value:?}"
        ));
    }
    if let Some(rejected) = value
        .chars()
        .find(|&c| matches!(c, ':' | '/' | '|' | '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}'))
    {
        return Err(format!(
            "{member} must not contain {rejected:?} (a colon, slash, vertical bar or \
             control character), got {value:?}"
        ));
    }
    if value == "arn" {
        return Err(format!(
            "{member} must not be the literal string \"arn\""
        ));
    }
    Ok(())
}

/// Refuse a `workflowExecutionRetentionPeriodInDays` outside the documented range.
///
/// The member's doc: "If you pass the value NONE or 0 (zero), then the workflow
/// execution history isn't retained. ... The maximum workflow execution retention
/// period is 90 days."
pub fn validate_retention_period(value: &str) -> Result<(), String> {
    if value == "NONE" {
        return Ok(());
    }
    match decimal(value) {
        Some(days) if days <= u64::from(MAX_RETENTION_PERIOD_IN_DAYS) => Ok(()),
        _ => Err(format!(
            "workflowExecutionRetentionPeriodInDays must be \"NONE\" or an integer from 0 to \
             {MAX_RETENTION_PERIOD_IN_DAYS}, got {value:?}"
        )),
    }
}

/// Refuse a decision-task start-to-close duration that is neither a non-negative
/// integer of seconds nor the literal `NONE`.
///
/// `defaultTaskStartToCloseTimeout`'s doc: "The duration is specified in seconds, an
/// integer greater than or equal to 0. You can use NONE to specify unlimited
/// duration."
pub fn validate_task_duration(member: &str, value: &str) -> Result<(), String> {
    if value == "NONE" || decimal(value).is_some() {
        return Ok(());
    }
    Err(format!(
        "{member} must be \"NONE\" or an integer number of seconds, got {value:?}"
    ))
}

/// Refuse an execution start-to-close duration outside what the model documents.
///
/// `executionStartToCloseTimeout`'s doc: "The duration is specified in seconds; an
/// integer greater than or equal to 0. Unlike some of the other timeout parameters in
/// Amazon SWF, you cannot specify a value of \"NONE\" for this timeout; there is a
/// one-year max limit on the time that a workflow execution can run."
pub fn validate_execution_duration(member: &str, value: &str) -> Result<(), String> {
    match decimal(value) {
        Some(seconds) if seconds <= MAX_EXECUTION_START_TO_CLOSE_SECONDS => Ok(()),
        _ => Err(format!(
            "{member} must be an integer number of seconds from 0 to \
             {MAX_EXECUTION_START_TO_CLOSE_SECONDS} (\"NONE\" is not accepted for this timeout), \
             got {value:?}"
        )),
    }
}

/// A plain decimal string as a number, or `None` when it is not one.
///
/// Every SWF duration is documented as "an integer greater than or equal to 0", so a
/// value must be decimal digits only: `str::parse` alone would also accept a signed
/// `"+5"`, which is not that form and which the service stores and echoes back
/// verbatim. Zero padding IS still that form (`"0007"` is seven), and the model
/// constrains only the member's length, so it is accepted.
fn decimal(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<u64>().ok()
}

/// Refuse a domain tag outside the documented character set.
///
/// `ResourceTag`'s own doc: "Tags may only contain unicode letters, digits,
/// whitespace, or these symbols: `_ . : / = + - @`". The key's and value's lengths are
/// modelled (1..128 and 0..256) and so are refused at the boundary; the character set
/// is not, so it is enforced here.
pub fn validate_tag(key: &str, value: Option<&str>) -> Result<(), String> {
    validate_tag_text("tags.key", key)?;
    match value {
        Some(value) => validate_tag_text("tags.value", value),
        None => Ok(()),
    }
}

/// One side of a tag pair, against the character set [`validate_tag`] documents.
fn validate_tag_text(member: &str, text: &str) -> Result<(), String> {
    if let Some(rejected) = text.chars().find(|c| {
        !(c.is_alphabetic()
            || c.is_numeric()
            || c.is_whitespace()
            || matches!(c, '_' | '.' | ':' | '/' | '=' | '+' | '-' | '@'))
    }) {
        return Err(format!(
            "{member} may only contain unicode letters, digits, whitespace or the symbols \
             _ . : / = + - @, got {rejected:?} in {text:?}"
        ));
    }
    Ok(())
}

/// Refuse a child policy outside the model's `ChildPolicy` enumeration.
///
/// Every `childPolicy` / `defaultChildPolicy` member targets that closed enum, so the
/// boundary validator already refuses anything outside it; this keeps a policy no
/// reader could interpret out of the store even if that trait stops being enforced,
/// and keeps the check identical on every handler that stores one.
pub fn validate_child_policy(member: &str, value: &str) -> Result<(), String> {
    if crate::ops::types::CHILD_POLICY_VALUES.contains(&value) {
        return Ok(());
    }
    Err(format!(
        "{member} must be one of {}, got {value:?}",
        crate::ops::types::CHILD_POLICY_VALUES.join(", ")
    ))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSwfSimpleworkflowserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSwfSimpleworkflowserviceState>)
}
