// SPDX-License-Identifier: CC0-1.0

//! @op StartWorkflowExecution
//!
//! Creates an OPEN execution under a freshly minted, stored `runId`, resolves its
//! effective configuration ONCE (the request value where given, else the workflow
//! type's registered default), and seeds its persisted history with
//! `WorkflowExecutionStarted` (eventId 1) followed by `DecisionTaskScheduled`
//! (eventId 2), leaving one open decision task.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `StartWorkflowExecution`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-swf-simpleworkflowservice/smithy-model/com-amazonaws-swf-simpleworkflowservice.json")
//! ```
//!
//! Starts an execution of the workflow type in the specified domain using the provided
//! workflowId and input data. This action returns the newly started workflow execution.
//! Access Control You can use IAM policies to control this action's access to Amazon SWF
//! resources as follows: Use a Resource element with the domain name to limit the action to
//! only specified domains. Use an Action element to allow or deny permission to call this
//! action. Constrain the following parameters by using a Condition element with the
//! appropriate keys. tagList.member.0: The key is swf:tagList.member.0. tagList.member.1:
//! The key is swf:tagList.member.1. tagList.member.2: The key is swf:tagList.member.2.
//! tagList.member.3: The key is swf:tagList.member.3. tagList.member.4: The key is
//! swf:tagList.member.4. taskList: String constraint. The key is swf:taskList.name.
//! workflowType.name: String constraint. The key is swf:workflowType.name.
//! workflowType.version: String constraint. The key is swf:workflowType.version. If the
//! caller doesn't have sufficient permissions to invoke the action, or the parameter values
//! fall outside the specified constraints, the action fails. The associated event
//! attribute's cause parameter is set to OPERATION_NOT_PERMITTED. For details and example
//! IAM policies, see Using IAM to Manage Access to Amazon SWF Workflows in the Amazon SWF
//! Developer Guide.
//!
//! ## Input
//!
//! * `childPolicy` (ChildPolicy, one of: ABANDON | REQUEST_CANCEL | TERMINATE) — If set,
//!   specifies the policy to use for the child workflow executions of this workflow execution
//!   if it is terminated, by calling the TerminateWorkflowExecution action explicitly or due
//!   to an expired timeout. This policy overrides the default child policy specified when
//!   registering the workflow type using RegisterWorkflowType. The supported child policies
//!   are: TERMINATE – The child executions are terminated. REQUEST_CANCEL – A request to
//!   cancel is attempted for each child execution by recording a
//!   WorkflowExecutionCancelRequested event in its history. It is up to the decider to take
//!   appropriate actions when it receives an execution history with this event. ABANDON – No
//!   action is taken. The …
//! * `domain` (DomainName, len 1..256) **required** — The name of the domain in which the
//!   workflow execution is created. The specified string must not contain a : (colon), /
//!   (slash), | (vertical bar), or any control characters (\u0000-\u001f | \u007f-\u009f).
//!   Also, it must not be the literal string arn.
//! * `executionStartToCloseTimeout` (DurationInSecondsOptional, len 0..8) — The total
//!   duration for this workflow execution. This overrides the
//!   defaultExecutionStartToCloseTimeout specified when registering the workflow type. The
//!   duration is specified in seconds; an integer greater than or equal to 0. Exceeding this
//!   limit causes the workflow execution to time out. Unlike some of the other timeout
//!   parameters in Amazon SWF, you cannot specify a value of "NONE" for this timeout; there
//!   is a one-year max limit on the time that a workflow execution can run. An execution
//!   start-to-close timeout must be specified either through this parameter or as a default
//!   when the workflow type is registered. If neither this parameter nor a default execution
//!   start-to-close timeout is …
//! * `input` (Data, len 0..32768) — The input for the workflow execution. This is a free form
//!   string which should be meaningful to the workflow you are starting. This input is made
//!   available to the new workflow execution in the WorkflowExecutionStarted history event.
//! * `lambdaRole` (Arn, len 1..1600) — The IAM role to attach to this workflow execution.
//!   Executions of this workflow type need IAM roles to invoke Lambda functions. If you don't
//!   attach an IAM role, any attempt to schedule a Lambda task fails. This results in a
//!   ScheduleLambdaFunctionFailed history event. For more information, see
//!   https://docs.aws.amazon.com/amazonswf/latest/developerguide/lambda-task.html in the
//!   Amazon SWF Developer Guide.
//! * `tagList` (TagList, len 0..5) — The list of tags to associate with the workflow
//!   execution. You can specify a maximum of 5 tags. You can list workflow executions with a
//!   specific tag by calling ListOpenWorkflowExecutions or ListClosedWorkflowExecutions and
//!   specifying a TagFilter.
//! * `taskList` (TaskList) — The task list to use for the decision tasks generated for this
//!   workflow execution. This overrides the defaultTaskList specified when registering the
//!   workflow type. A task list for this workflow execution must be specified either as a
//!   default for the workflow type or through this parameter. If neither this parameter is
//!   set nor a default task list was specified at registration time then a fault is returned.
//!   The specified string must not contain a : (colon), / (slash), | (vertical bar), or any
//!   control characters (\u0000-\u001f | \u007f-\u009f). Also, it must not be the literal
//!   string arn.
//! * `taskPriority` (TaskPriority) — The task priority to use for this workflow execution.
//!   This overrides any default priority that was assigned when the workflow type was
//!   registered. If not set, then the default task priority for the workflow type is used.
//!   Valid values are integers that range from Java's Integer.MIN_VALUE (-2147483648) to
//!   Integer.MAX_VALUE (2147483647). Higher numbers indicate higher priority. For more
//!   information about setting task priority, see Setting Task Priority in the Amazon SWF
//!   Developer Guide.
//! * `taskStartToCloseTimeout` (DurationInSecondsOptional, len 0..8) — Specifies the maximum
//!   duration of decision tasks for this workflow execution. This parameter overrides the
//!   defaultTaskStartToCloseTimout specified when registering the workflow type using
//!   RegisterWorkflowType. The duration is specified in seconds, an integer greater than or
//!   equal to 0. You can use NONE to specify unlimited duration. A task start-to-close
//!   timeout for this workflow execution must be specified either as a default for the
//!   workflow type or through this parameter. If neither this parameter is set nor a default
//!   task start-to-close timeout was specified at registration time then a fault is returned.
//! * `workflowId` (WorkflowId, len 1..256) **required** — The user defined identifier
//!   associated with the workflow execution. You can use this to associate a custom
//!   identifier with the workflow execution. You may specify the same identifier if a
//!   workflow execution is logically a restart of a previous execution. You cannot have two
//!   open workflow executions with the same workflowId at the same time within the same
//!   domain. The specified string must not contain a : (colon), / (slash), | (vertical bar),
//!   or any control characters (\u0000-\u001f | \u007f-\u009f). Also, it must not be the
//!   literal string arn.
//! * `workflowType` (WorkflowType) **required** — The type of the workflow to start.
//!
//! ## Output
//!
//! * `runId` (WorkflowRunId, len 1..64) — The runId of a workflow execution. This ID is
//!   generated by the service and can be used to uniquely identify the workflow execution
//!   within a domain.
//!
//! ## Errors
//!
//! * `DefaultUndefinedFault` (HTTP 400) — The StartWorkflowExecution API action was called
//!   without the required parameters set. Some workflow execution parameters, such as the
//!   decision taskList, must be set to start the execution. However, these parameters might
//!   have been set as defaults when the workflow type was registered. In this case, you can
//!   omit these parameters from the StartWorkflowExecution call and Amazon SWF uses the
//!   values defined in the workflow type. If these parameters aren't set and no default
//!   parameters were defined in the workflow type, this error is displayed.
//! * `LimitExceededFault` (HTTP 400) — Returned by any operation if a system imposed
//!   limitation has been reached. To address this fault you should either clean up unused
//!   resources or increase the limit by contacting AWS.
//! * `OperationNotPermittedFault` (HTTP 400) — Returned when the caller doesn't have
//!   sufficient permissions to invoke the action.
//! * `TypeDeprecatedFault` (HTTP 400) — Returned when the specified activity or workflow type
//!   was already deprecated.
//! * `UnknownResourceFault` (HTTP 400) — Returned when the named resource cannot be found
//!   with in the scope of this operation (region or domain). This could happen if the named
//!   resource was never created or is no longer available for this operation.
//! * `WorkflowExecutionAlreadyStartedFault` (HTTP 400) — Returned by StartWorkflowExecution
//!   when an open execution with the same workflowId is already running in the specified
//!   domain.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `StartWorkflowExecutionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-swf-simpleworkflowservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StartWorkflowExecutionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.swf#StartWorkflowExecution")]
#[derive(Debug)]
pub enum StartWorkflowExecutionErrorModeled {
    DefaultUndefinedFault(DefaultUndefinedFault),
    LimitExceededFault(LimitExceededFault),
    OperationNotPermittedFault(OperationNotPermittedFault),
    TypeDeprecatedFault(TypeDeprecatedFault),
    UnknownResourceFault(UnknownResourceFault),
    WorkflowExecutionAlreadyStartedFault(WorkflowExecutionAlreadyStartedFault),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_workflow_execution::StartWorkflowExecutionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StartWorkflowExecutionError = crate::scaffolded::Scaffolded<StartWorkflowExecutionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StartWorkflowExecutionErrorModeled> for StartWorkflowExecutionError {
    fn from(modeled: StartWorkflowExecutionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartWorkflowExecution` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StartWorkflowExecution<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartWorkflowExecution<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartWorkflowExecution<Eng>
{
    type Input = StartWorkflowExecutionInput;
    type Output = StartWorkflowExecutionOutput;
    type Error = StartWorkflowExecutionError;

    const NAME: &'static str = "StartWorkflowExecution";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_WORKFLOW_EXECUTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_WORKFLOW_EXECUTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            StartWorkflowExecutionErrorModeled::OperationNotPermittedFault(
                OperationNotPermittedFault {
                    message: Some("the SWF domain store is unavailable".to_string()),
                },
            )
        })?;

        crate::state::validate_name("workflowId", &input.workflow_id)
            .map_err(crate::scaffolded::Scaffolded::validation)?;
        crate::state::validate_name("workflowType.name", &input.workflow_type.name)
            .map_err(crate::scaffolded::Scaffolded::validation)?;
        crate::state::validate_name("workflowType.version", &input.workflow_type.version)
            .map_err(crate::scaffolded::Scaffolded::validation)?;

        let requested_task_list = match &input.task_list {
            Some(task_list) if !task_list.name.is_empty() => {
                crate::state::validate_name("taskList.name", &task_list.name)
                    .map_err(crate::scaffolded::Scaffolded::validation)?;
                Some(task_list.name.clone())
            }
            _ => None,
        };
        let requested_task_timeout = crate::state::non_empty(&input.task_start_to_close_timeout);
        if let Some(timeout) = &requested_task_timeout {
            crate::state::validate_task_duration("taskStartToCloseTimeout", timeout)
                .map_err(crate::scaffolded::Scaffolded::validation)?;
        }
        let requested_execution_timeout =
            crate::state::non_empty(&input.execution_start_to_close_timeout);
        if let Some(timeout) = &requested_execution_timeout {
            crate::state::validate_execution_duration("executionStartToCloseTimeout", timeout)
                .map_err(crate::scaffolded::Scaffolded::validation)?;
        }
        let requested_child_policy = crate::state::non_empty(&input.child_policy);
        if let Some(policy) = &requested_child_policy {
            crate::state::validate_child_policy("childPolicy", policy)
                .map_err(crate::scaffolded::Scaffolded::validation)?;
        }

        let mut guard = state.lock();
        // A `workflowId` whose previous run the clock already closed is startable
        // again, so the service's own start-to-close reaping is materialised before the
        // already-started guard below reads the open-run index.
        guard.materialise_due_timeouts_in(&input.domain, crate::state::now());
        let Some(domain) = guard.domains.get_mut(&input.domain) else {
            return Err(StartWorkflowExecutionErrorModeled::UnknownResourceFault(
                UnknownResourceFault {
                    message: Some(crate::state::unknown_domain_message(&input.domain)),
                },
            )
            .into());
        };

        let type_key = (
            input.workflow_type.name.clone(),
            input.workflow_type.version.clone(),
        );
        let Some(workflow_type) = domain.workflow_types.get(&type_key).cloned() else {
            return Err(StartWorkflowExecutionErrorModeled::UnknownResourceFault(
                UnknownResourceFault {
                    message: Some(crate::state::unknown_workflow_type_message(
                        &input.workflow_type.name,
                        &input.workflow_type.version,
                    )),
                },
            )
            .into());
        };
        if workflow_type.status != crate::state::STATUS_REGISTERED {
            return Err(StartWorkflowExecutionErrorModeled::TypeDeprecatedFault(
                TypeDeprecatedFault {
                    message: Some(format!(
                        "WorkflowType=[name={}, version={}]",
                        workflow_type.name, workflow_type.version
                    )),
                },
            )
            .into());
        }

        // One OPEN run per `workflowId` per domain: a start that collides with a run
        // that is still open is the modelled already-started fault, never a second
        // open run under the same id.
        //
        // The fault IDENTIFIES the incumbent: besides the code, it carries the
        // `workflowExecution` of the run that holds the id — the requested `workflowId`
        // with the `runId` taken from this service's own open-run index — which is what
        // lets a caller that collided describe, signal or terminate that run without a
        // separate visibility query. The recorded service answer pairs that identity
        // with an EMPTY message, present rather than absent; do not "simplify" the
        // message to `None`.
        if let Some(open_run_id) = domain.open_by_workflow_id.get(&input.workflow_id) {
            return Err(
                StartWorkflowExecutionErrorModeled::WorkflowExecutionAlreadyStartedFault(
                    WorkflowExecutionAlreadyStartedFault {
                        message: Some(String::new()),
                        workflow_execution: Some(WorkflowExecution {
                            workflow_id: input.workflow_id.clone(),
                            run_id: open_run_id.clone(),
                        }),
                    },
                )
                .into(),
            );
        }

        if domain.executions.len() >= crate::state::MAX_EXECUTIONS_PER_DOMAIN {
            return Err(StartWorkflowExecutionErrorModeled::LimitExceededFault(
                LimitExceededFault {
                    message: Some(format!(
                        "this emulator retains at most {} workflow executions per domain",
                        crate::state::MAX_EXECUTIONS_PER_DOMAIN
                    )),
                },
            )
            .into());
        }

        // Each of these four must be defined either on the request or as a
        // registration-time default; when neither has it, the start is refused with
        // the modelled DefaultUndefinedFault ("The StartWorkflowExecution API action
        // was called without the required parameters set").
        let undefined = |member: &str, default_member: &str| {
            StartWorkflowExecutionErrorModeled::DefaultUndefinedFault(DefaultUndefinedFault {
                message: Some(format!(
                    "{member} was not set on the request and the workflow type defines no \
                     {default_member}"
                )),
            })
        };
        let task_list = requested_task_list
            .or_else(|| workflow_type.default_task_list.clone())
            .ok_or_else(|| undefined("taskList", "defaultTaskList"))?;
        let task_start_to_close_timeout = requested_task_timeout
            .or_else(|| workflow_type.default_task_start_to_close_timeout.clone())
            .ok_or_else(|| {
                undefined("taskStartToCloseTimeout", "defaultTaskStartToCloseTimeout")
            })?;
        let execution_start_to_close_timeout = requested_execution_timeout
            .or_else(|| {
                workflow_type
                    .default_execution_start_to_close_timeout
                    .clone()
            })
            .ok_or_else(|| {
                undefined(
                    "executionStartToCloseTimeout",
                    "defaultExecutionStartToCloseTimeout",
                )
            })?;
        let child_policy = requested_child_policy
            .or_else(|| workflow_type.default_child_policy.clone())
            .ok_or_else(|| undefined("childPolicy", "defaultChildPolicy"))?;

        let effective_config = crate::state::ExecutionConfig {
            task_list,
            task_priority: crate::state::non_empty(&input.task_priority)
                .or_else(|| workflow_type.default_task_priority.clone()),
            task_start_to_close_timeout,
            execution_start_to_close_timeout,
            child_policy,
            lambda_role: crate::state::non_empty(&input.lambda_role)
                .or_else(|| workflow_type.default_lambda_role.clone()),
        };

        let started_at = crate::state::now();
        // The ONE place a run comes into existence: the stored record, its
        // `WorkflowExecutionStarted` event and the first decision task SWF schedules as
        // soon as an execution starts. A `ContinueAsNewWorkflowExecution` or
        // `StartChildWorkflowExecution` decision starts its run through the same path,
        // which is also where the per-domain run bound is enforced — the check above is
        // this operation's own early refusal of a domain that is already full.
        let run_id = domain
            .start_run(
                crate::state::NewRun {
                    workflow_id: input.workflow_id.clone(),
                    workflow_type: workflow_type.workflow_type(),
                    input: input.input.clone(),
                    tag_list: input.tag_list.clone().unwrap_or_default(),
                    config: effective_config,
                    // Only a decider's continue-as-new or a parent's child start carries
                    // either of these, and this call is a client's own start.
                    continued_execution_run_id: None,
                    parent: None,
                },
                started_at,
            )
            .ok_or_else(|| {
                StartWorkflowExecutionErrorModeled::LimitExceededFault(LimitExceededFault {
                    message: Some(format!(
                        "this emulator retains at most {} workflow executions per domain",
                        crate::state::MAX_EXECUTIONS_PER_DOMAIN
                    )),
                })
            })?;

        Ok(StartWorkflowExecutionOutput {
            run_id: Some(run_id),
        })
    }
}
