// SPDX-License-Identifier: CC0-1.0

//! @op TerminateWorkflowExecution
//!
//! Closes the stored run — `executionStatus` CLOSED, `closeStatus` TERMINATED, a
//! `closeTimestamp` — appends a `WorkflowExecutionTerminated` event to its history and
//! clears its open tasks. The output body is empty (the output shape is `Unit`), so
//! the stored record is the only observable effect.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `TerminateWorkflowExecution`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-swf-simpleworkflowservice/smithy-model/com-amazonaws-swf-simpleworkflowservice.json")
//! ```
//!
//! Records a WorkflowExecutionTerminated event and forces closure of the workflow execution
//! identified by the given domain, runId, and workflowId. The child policy, registered with
//! the workflow type or specified when starting this execution, is applied to any open
//! child workflow executions of this workflow execution. If the identified workflow
//! execution was in progress, it is terminated immediately. If a runId isn't specified,
//! then the WorkflowExecutionTerminated event is recorded in the history of the current
//! open workflow with the matching workflowId in the domain. You should consider using
//! RequestCancelWorkflowExecution action instead because it allows the workflow to
//! gracefully close while TerminateWorkflowExecution doesn't. Access Control You can use
//! IAM policies to control this action's access to Amazon SWF resources as follows: Use a
//! Resource element with the domain name to limit the action to only specified domains. Use
//! an Action element to allow or deny permission to call this action. You cannot use an IAM
//! policy to constrain this action's parameters. If the caller doesn't have sufficient
//! permissions to invoke the action, or the parameter values fall outside the specified
//! constraints, the action fails. The associated event attribute's cause parameter is set
//! to OPERATION_NOT_PERMITTED. For details and example IAM policies, see Using IAM to
//! Manage Access to Amazon SWF Workflows in the Amazon SWF Developer Guide.
//!
//! ## Input
//!
//! * `childPolicy` (ChildPolicy, one of: ABANDON | REQUEST_CANCEL | TERMINATE) — If set,
//!   specifies the policy to use for the child workflow executions of the workflow execution
//!   being terminated. This policy overrides the child policy specified for the workflow
//!   execution at registration time or when starting the execution. The supported child
//!   policies are: TERMINATE – The child executions are terminated. REQUEST_CANCEL – A
//!   request to cancel is attempted for each child execution by recording a
//!   WorkflowExecutionCancelRequested event in its history. It is up to the decider to take
//!   appropriate actions when it receives an execution history with this event. ABANDON – No
//!   action is taken. The child executions continue to run. A child policy for this workflow
//!   execution must be …
//! * `details` (Data, len 0..32768) — Details for terminating the workflow execution.
//! * `domain` (DomainName, len 1..256) **required** — The domain of the workflow execution to
//!   terminate.
//! * `reason` (TerminateReason, len 0..256) — A descriptive reason for terminating the
//!   workflow execution.
//! * `runId` (WorkflowRunIdOptional, len 0..64) — The runId of the workflow execution to
//!   terminate.
//! * `workflowId` (WorkflowId, len 1..256) **required** — The workflowId of the workflow
//!   execution to terminate.
//!
//! ## Errors
//!
//! * `OperationNotPermittedFault` (HTTP 400) — Returned when the caller doesn't have
//!   sufficient permissions to invoke the action.
//! * `UnknownResourceFault` (HTTP 400) — Returned when the named resource cannot be found
//!   with in the scope of this operation (region or domain). This could happen if the named
//!   resource was never created or is no longer available for this operation.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `TerminateWorkflowExecutionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-swf-simpleworkflowservice.json");

#[derive(Debug, Clone, Default)]
pub struct TerminateWorkflowExecutionOutput;

impl TerminateWorkflowExecutionOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static TERMINATE_WORKFLOW_EXECUTION_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &TerminateWorkflowExecutionOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for TerminateWorkflowExecutionOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TerminateWorkflowExecutionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.swf#TerminateWorkflowExecution")]
#[derive(Debug)]
pub enum TerminateWorkflowExecutionErrorModeled {
    OperationNotPermittedFault(OperationNotPermittedFault),
    UnknownResourceFault(UnknownResourceFault),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::terminate_workflow_execution::TerminateWorkflowExecutionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type TerminateWorkflowExecutionError = crate::scaffolded::Scaffolded<TerminateWorkflowExecutionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<TerminateWorkflowExecutionErrorModeled> for TerminateWorkflowExecutionError {
    fn from(modeled: TerminateWorkflowExecutionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `TerminateWorkflowExecution` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct TerminateWorkflowExecution<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> TerminateWorkflowExecution<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for TerminateWorkflowExecution<Eng>
{
    type Input = TerminateWorkflowExecutionInput;
    type Output = TerminateWorkflowExecutionOutput;
    type Error = TerminateWorkflowExecutionError;

    const NAME: &'static str = "TerminateWorkflowExecution";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TERMINATE_WORKFLOW_EXECUTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = TERMINATE_WORKFLOW_EXECUTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            TerminateWorkflowExecutionErrorModeled::OperationNotPermittedFault(
                OperationNotPermittedFault {
                    message: Some("the SWF domain store is unavailable".to_string()),
                },
            )
        })?;

        crate::state::validate_name("workflowId", &input.workflow_id)
            .map_err(crate::scaffolded::Scaffolded::validation)?;

        let requested_child_policy = crate::state::non_empty(&input.child_policy);
        if let Some(policy) = &requested_child_policy {
            crate::state::validate_child_policy("childPolicy", policy)
                .map_err(crate::scaffolded::Scaffolded::validation)?;
        }

        let closed_at = crate::state::now();
        let mut guard = state.lock();
        // A run the clock already closed is out of scope for a terminate, so the
        // service's own start-to-close reaping is materialised before the lookup.
        guard.materialise_due_timeouts_in(&input.domain, closed_at);
        let unknown = || {
            TerminateWorkflowExecutionErrorModeled::UnknownResourceFault(UnknownResourceFault {
                message: Some(format!(
                    "Unknown execution, workflowId = {}",
                    input.workflow_id
                )),
            })
        };
        let Some(domain) = guard.domains.get_mut(&input.domain) else {
            return Err(unknown().into());
        };
        // An omitted `runId` addresses the one run of this `workflowId` that is still
        // open — which is also why terminating an already-closed run resolves to
        // nothing and is an unknown resource.
        let Some(run_id) = domain.resolve_run_id(&input.workflow_id, input.run_id.as_deref())
        else {
            return Err(unknown().into());
        };
        let Some(execution) = domain
            .executions
            .get_mut(&(input.workflow_id.clone(), run_id.clone()))
        else {
            return Err(unknown().into());
        };
        if !execution.is_open() {
            return Err(unknown().into());
        }

        let child_policy = requested_child_policy
            .unwrap_or_else(|| execution.effective_config.child_policy.clone());

        // The ONE place a run closes: the close event and `closeStatus`, the `workflowId`
        // released for a new start, the child policy applied to the children this run
        // leaves behind (`childPolicy` "specifies the policy to use for the child
        // workflow executions of this workflow execution if it is terminated"), and the
        // parent run told what became of it.
        domain.close_run(
            &(input.workflow_id.clone(), run_id.clone()),
            crate::state::RunClose::Terminated {
                reason: input.reason.clone(),
                details: input.details.clone(),
                // `cause` names what terminated the run: this call is an operator's own,
                // which is exactly the cause `WorkflowExecutionTerminatedCause` models
                // for it, beside the service's own `CHILD_POLICY_APPLIED` and
                // `EVENT_LIMIT_EXCEEDED`.
                cause: Some(crate::state::TERMINATED_CAUSE_OPERATOR_INITIATED.to_string()),
                child_policy: Some(child_policy),
            },
            closed_at,
        );

        Ok(TerminateWorkflowExecutionOutput)
    }
}
