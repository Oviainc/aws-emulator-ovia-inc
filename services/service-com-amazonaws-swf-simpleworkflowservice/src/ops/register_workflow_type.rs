// SPDX-License-Identifier: CC0-1.0

//! @op RegisterWorkflowType
//!
//! Stores a workflow type under `(domain, name, version)` with status `REGISTERED`
//! and the registration-time defaults `StartWorkflowExecution` falls back to. The
//! output body is empty (the output shape is `Unit`), so the stored record is the only
//! observable effect.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `RegisterWorkflowType`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-swf-simpleworkflowservice/smithy-model/com-amazonaws-swf-simpleworkflowservice.json")
//! ```
//!
//! Registers a new workflow type and its configuration settings in the specified domain.
//! The retention period for the workflow history is set by the RegisterDomain action. If
//! the type already exists, then a TypeAlreadyExists fault is returned. You cannot change
//! the configuration settings of a workflow type once it is registered and it must be
//! registered as a new version. Access Control You can use IAM policies to control this
//! action's access to Amazon SWF resources as follows: Use a Resource element with the
//! domain name to limit the action to only specified domains. Use an Action element to
//! allow or deny permission to call this action. Constrain the following parameters by
//! using a Condition element with the appropriate keys. defaultTaskList.name: String
//! constraint. The key is swf:defaultTaskList.name. name: String constraint. The key is
//! swf:name. version: String constraint. The key is swf:version. If the caller doesn't have
//! sufficient permissions to invoke the action, or the parameter values fall outside the
//! specified constraints, the action fails. The associated event attribute's cause
//! parameter is set to OPERATION_NOT_PERMITTED. For details and example IAM policies, see
//! Using IAM to Manage Access to Amazon SWF Workflows in the Amazon SWF Developer Guide.
//!
//! ## Input
//!
//! * `defaultChildPolicy` (ChildPolicy, one of: ABANDON | REQUEST_CANCEL | TERMINATE) — If
//!   set, specifies the default policy to use for the child workflow executions when a
//!   workflow execution of this type is terminated, by calling the TerminateWorkflowExecution
//!   action explicitly or due to an expired timeout. This default can be overridden when
//!   starting a workflow execution using the StartWorkflowExecution action or the
//!   StartChildWorkflowExecution Decision. The supported child policies are: TERMINATE – The
//!   child executions are terminated. REQUEST_CANCEL – A request to cancel is attempted for
//!   each child execution by recording a WorkflowExecutionCancelRequested event in its
//!   history. It is up to the decider to take appropriate actions when it receives an
//!   execution history with …
//! * `defaultExecutionStartToCloseTimeout` (DurationInSecondsOptional, len 0..8) — If set,
//!   specifies the default maximum duration for executions of this workflow type. You can
//!   override this default when starting an execution through the StartWorkflowExecution
//!   Action or StartChildWorkflowExecution Decision. The duration is specified in seconds; an
//!   integer greater than or equal to 0. Unlike some of the other timeout parameters in
//!   Amazon SWF, you cannot specify a value of "NONE" for
//!   defaultExecutionStartToCloseTimeout; there is a one-year max limit on the time that a
//!   workflow execution can run. Exceeding this limit always causes the workflow execution to
//!   time out.
//! * `defaultLambdaRole` (Arn, len 1..1600) — The default IAM role attached to this workflow
//!   type. Executions of this workflow type need IAM roles to invoke Lambda functions. If you
//!   don't specify an IAM role when you start this workflow type, the default Lambda role is
//!   attached to the execution. For more information, see
//!   https://docs.aws.amazon.com/amazonswf/latest/developerguide/lambda-task.html in the
//!   Amazon SWF Developer Guide.
//! * `defaultTaskList` (TaskList) — If set, specifies the default task list to use for
//!   scheduling decision tasks for executions of this workflow type. This default is used
//!   only if a task list isn't provided when starting the execution through the
//!   StartWorkflowExecution Action or StartChildWorkflowExecution Decision.
//! * `defaultTaskPriority` (TaskPriority) — The default task priority to assign to the
//!   workflow type. If not assigned, then 0 is used. Valid values are integers that range
//!   from Java's Integer.MIN_VALUE (-2147483648) to Integer.MAX_VALUE (2147483647). Higher
//!   numbers indicate higher priority. For more information about setting task priority, see
//!   Setting Task Priority in the Amazon SWF Developer Guide.
//! * `defaultTaskStartToCloseTimeout` (DurationInSecondsOptional, len 0..8) — If set,
//!   specifies the default maximum duration of decision tasks for this workflow type. This
//!   default can be overridden when starting a workflow execution using the
//!   StartWorkflowExecution action or the StartChildWorkflowExecution Decision. The duration
//!   is specified in seconds, an integer greater than or equal to 0. You can use NONE to
//!   specify unlimited duration.
//! * `description` (Description, len 0..1024) — Textual description of the workflow type.
//! * `domain` (DomainName, len 1..256) **required** — The name of the domain in which to
//!   register the workflow type.
//! * `name` (Name, len 1..256) **required** — The name of the workflow type. The specified
//!   string must not contain a : (colon), / (slash), | (vertical bar), or any control
//!   characters (\u0000-\u001f | \u007f-\u009f). Also, it must not be the literal string arn.
//! * `version` (Version, len 1..64) **required** — The version of the workflow type. The
//!   workflow type consists of the name and version, the combination of which must be unique
//!   within the domain. To get a list of all currently registered workflow types, use the
//!   ListWorkflowTypes action. The specified string must not contain a : (colon), / (slash),
//!   | (vertical bar), or any control characters (\u0000-\u001f | \u007f-\u009f). Also, it
//!   must not be the literal string arn.
//!
//! ## Errors
//!
//! * `LimitExceededFault` (HTTP 400) — Returned by any operation if a system imposed
//!   limitation has been reached. To address this fault you should either clean up unused
//!   resources or increase the limit by contacting AWS.
//! * `OperationNotPermittedFault` (HTTP 400) — Returned when the caller doesn't have
//!   sufficient permissions to invoke the action.
//! * `TypeAlreadyExistsFault` (HTTP 400) — Returned if the type already exists in the
//!   specified domain. You may get this fault if you are registering a type that is either
//!   already registered or deprecated, or if you undeprecate a type that is currently
//!   registered.
//! * `UnknownResourceFault` (HTTP 400) — Returned when the named resource cannot be found
//!   with in the scope of this operation (region or domain). This could happen if the named
//!   resource was never created or is no longer available for this operation.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `RegisterWorkflowTypeError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-swf-simpleworkflowservice.json");

#[derive(Debug, Clone, Default)]
pub struct RegisterWorkflowTypeOutput;

impl RegisterWorkflowTypeOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static REGISTER_WORKFLOW_TYPE_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &RegisterWorkflowTypeOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for RegisterWorkflowTypeOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `RegisterWorkflowTypeError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.swf#RegisterWorkflowType")]
#[derive(Debug)]
pub enum RegisterWorkflowTypeErrorModeled {
    LimitExceededFault(LimitExceededFault),
    OperationNotPermittedFault(OperationNotPermittedFault),
    TypeAlreadyExistsFault(TypeAlreadyExistsFault),
    UnknownResourceFault(UnknownResourceFault),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::register_workflow_type::RegisterWorkflowTypeError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type RegisterWorkflowTypeError = crate::scaffolded::Scaffolded<RegisterWorkflowTypeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<RegisterWorkflowTypeErrorModeled> for RegisterWorkflowTypeError {
    fn from(modeled: RegisterWorkflowTypeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `RegisterWorkflowType` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct RegisterWorkflowType<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> RegisterWorkflowType<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for RegisterWorkflowType<Eng>
{
    type Input = RegisterWorkflowTypeInput;
    type Output = RegisterWorkflowTypeOutput;
    type Error = RegisterWorkflowTypeError;

    const NAME: &'static str = "RegisterWorkflowType";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REGISTER_WORKFLOW_TYPE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = REGISTER_WORKFLOW_TYPE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            RegisterWorkflowTypeErrorModeled::OperationNotPermittedFault(
                OperationNotPermittedFault {
                    message: Some("the SWF domain store is unavailable".to_string()),
                },
            )
        })?;

        crate::state::validate_name("name", &input.name)
            .map_err(crate::scaffolded::Scaffolded::validation)?;
        crate::state::validate_name("version", &input.version)
            .map_err(crate::scaffolded::Scaffolded::validation)?;
        let default_task_list = match &input.default_task_list {
            // An empty `name` is no task list at all, the same reading
            // `StartWorkflowExecution` applies to its own `taskList` — so a stored
            // default can never be a value the start path would ignore.
            Some(task_list) if !task_list.name.is_empty() => {
                crate::state::validate_name("defaultTaskList.name", &task_list.name)
                    .map_err(crate::scaffolded::Scaffolded::validation)?;
                Some(task_list.name.clone())
            }
            _ => None,
        };

        let default_task_start_to_close_timeout =
            crate::state::non_empty(&input.default_task_start_to_close_timeout);
        if let Some(timeout) = &default_task_start_to_close_timeout {
            crate::state::validate_task_duration("defaultTaskStartToCloseTimeout", timeout)
                .map_err(crate::scaffolded::Scaffolded::validation)?;
        }
        let default_execution_start_to_close_timeout =
            crate::state::non_empty(&input.default_execution_start_to_close_timeout);
        if let Some(timeout) = &default_execution_start_to_close_timeout {
            crate::state::validate_execution_duration(
                "defaultExecutionStartToCloseTimeout",
                timeout,
            )
            .map_err(crate::scaffolded::Scaffolded::validation)?;
        }

        let default_child_policy = crate::state::non_empty(&input.default_child_policy);
        if let Some(policy) = &default_child_policy {
            crate::state::validate_child_policy("defaultChildPolicy", policy)
                .map_err(crate::scaffolded::Scaffolded::validation)?;
        }

        let mut guard = state.lock();
        let Some(domain) = guard.domains.get_mut(&input.domain) else {
            return Err(RegisterWorkflowTypeErrorModeled::UnknownResourceFault(
                UnknownResourceFault {
                    message: Some(crate::state::unknown_domain_message(&input.domain)),
                },
            )
            .into());
        };

        let key = (input.name.clone(), input.version.clone());
        if domain.workflow_types.contains_key(&key) {
            return Err(RegisterWorkflowTypeErrorModeled::TypeAlreadyExistsFault(
                TypeAlreadyExistsFault {
                    message: Some(format!(
                        "WorkflowType=[name={}, version={}]",
                        input.name, input.version
                    )),
                },
            )
            .into());
        }
        if domain.workflow_types.len() >= crate::state::MAX_WORKFLOW_TYPES_PER_DOMAIN {
            return Err(RegisterWorkflowTypeErrorModeled::LimitExceededFault(
                LimitExceededFault {
                    message: Some(format!(
                        "this emulator holds at most {} workflow types per domain",
                        crate::state::MAX_WORKFLOW_TYPES_PER_DOMAIN
                    )),
                },
            )
            .into());
        }

        domain.workflow_types.insert(
            key,
            crate::state::WorkflowTypeRecord {
                name: input.name.clone(),
                version: input.version.clone(),
                description: input.description.clone(),
                status: crate::state::STATUS_REGISTERED.to_string(),
                creation_date: crate::state::now(),
                deprecation_date: None,
                default_task_list,
                default_task_start_to_close_timeout,
                default_execution_start_to_close_timeout,
                default_task_priority: crate::state::non_empty(&input.default_task_priority),
                default_child_policy,
                default_lambda_role: crate::state::non_empty(&input.default_lambda_role),
            },
        );

        Ok(RegisterWorkflowTypeOutput)
    }
}
