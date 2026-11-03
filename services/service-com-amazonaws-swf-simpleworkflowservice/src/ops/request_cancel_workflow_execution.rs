// SPDX-License-Identifier: CC0-1.0

//! @op RequestCancelWorkflowExecution
//!
//! Records a `WorkflowExecutionCancelRequested` event in the addressed OPEN run and
//! marks that stored run as having a cancellation requested — the run stays OPEN,
//! because it is the decider that decides how (and whether) to close it. The output
//! body is empty (the output shape is `Unit`), so the stored record is the only
//! observable effect: `DescribeWorkflowExecution` reports `cancelRequested` and
//! `GetWorkflowExecutionHistory` reports the appended event.
//!
//! The request is a latch, so a run carries at most ONE such event: a repeated request
//! against a run already carrying one is accepted and appends nothing, which also keeps
//! a caller from growing the stored history once per call for as long as a run stays
//! open.
//!
//! The event is one "the decider [must] take appropriate actions when it receives an
//! execution history with", and a decider only ever receives a history through a
//! decision task, so the request also makes that reaction pollable: it schedules a
//! decision task on the run's effective task list. SWF keeps at most one decision task
//! outstanding per execution, so a run that already has one in flight gets no second —
//! the history that task's decider is handed carries this event anyway.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `RequestCancelWorkflowExecution`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-swf-simpleworkflowservice/smithy-model/com-amazonaws-swf-simpleworkflowservice.json")
//! ```
//!
//! Records a WorkflowExecutionCancelRequested event in the currently running workflow
//! execution identified by the given domain, workflowId, and runId. This logically requests
//! the cancellation of the workflow execution as a whole. It is up to the decider to take
//! appropriate actions when it receives an execution history with this event. If the runId
//! isn't specified, the WorkflowExecutionCancelRequested event is recorded in the history
//! of the current open workflow execution with the specified workflowId in the domain.
//! Because this action allows the workflow to properly clean up and gracefully close, it
//! should be used instead of TerminateWorkflowExecution when possible. Access Control You
//! can use IAM policies to control this action's access to Amazon SWF resources as follows:
//! Use a Resource element with the domain name to limit the action to only specified
//! domains. Use an Action element to allow or deny permission to call this action. You
//! cannot use an IAM policy to constrain this action's parameters. If the caller doesn't
//! have sufficient permissions to invoke the action, or the parameter values fall outside
//! the specified constraints, the action fails. The associated event attribute's cause
//! parameter is set to OPERATION_NOT_PERMITTED. For details and example IAM policies, see
//! Using IAM to Manage Access to Amazon SWF Workflows in the Amazon SWF Developer Guide.
//!
//! ## Input
//!
//! * `domain` (DomainName, len 1..256) **required** — The name of the domain containing the
//!   workflow execution to cancel.
//! * `runId` (WorkflowRunIdOptional, len 0..64) — The runId of the workflow execution to
//!   cancel.
//! * `workflowId` (WorkflowId, len 1..256) **required** — The workflowId of the workflow
//!   execution to cancel.
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
// `RequestCancelWorkflowExecutionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-swf-simpleworkflowservice.json");

#[derive(Debug, Clone, Default)]
pub struct RequestCancelWorkflowExecutionOutput;

impl RequestCancelWorkflowExecutionOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static REQUEST_CANCEL_WORKFLOW_EXECUTION_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &RequestCancelWorkflowExecutionOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for RequestCancelWorkflowExecutionOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `RequestCancelWorkflowExecutionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.swf#RequestCancelWorkflowExecution")]
#[derive(Debug)]
pub enum RequestCancelWorkflowExecutionErrorModeled {
    OperationNotPermittedFault(OperationNotPermittedFault),
    UnknownResourceFault(UnknownResourceFault),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::request_cancel_workflow_execution::RequestCancelWorkflowExecutionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type RequestCancelWorkflowExecutionError = crate::scaffolded::Scaffolded<RequestCancelWorkflowExecutionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<RequestCancelWorkflowExecutionErrorModeled> for RequestCancelWorkflowExecutionError {
    fn from(modeled: RequestCancelWorkflowExecutionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `RequestCancelWorkflowExecution` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct RequestCancelWorkflowExecution<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> RequestCancelWorkflowExecution<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for RequestCancelWorkflowExecution<Eng>
{
    type Input = RequestCancelWorkflowExecutionInput;
    type Output = RequestCancelWorkflowExecutionOutput;
    type Error = RequestCancelWorkflowExecutionError;

    const NAME: &'static str = "RequestCancelWorkflowExecution";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REQUEST_CANCEL_WORKFLOW_EXECUTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = REQUEST_CANCEL_WORKFLOW_EXECUTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            RequestCancelWorkflowExecutionErrorModeled::OperationNotPermittedFault(
                OperationNotPermittedFault {
                    message: Some("the SWF domain store is unavailable".to_string()),
                },
            )
        })?;

        crate::state::validate_name("workflowId", &input.workflow_id)
            .map_err(crate::scaffolded::Scaffolded::validation)?;

        let requested_at = crate::state::now();
        let mut guard = state.lock();
        // A run the clock already closed is out of scope for a cancel request, so the
        // service's own start-to-close reaping is materialised before the lookup.
        guard.materialise_due_timeouts_in(&input.domain, requested_at);
        let unknown = || {
            RequestCancelWorkflowExecutionErrorModeled::UnknownResourceFault(
                UnknownResourceFault {
                    message: Some(format!(
                        "Unknown execution, workflowId = {}",
                        input.workflow_id
                    )),
                },
            )
        };
        let Some(domain) = guard.domains.get_mut(&input.domain) else {
            return Err(unknown().into());
        };
        // An omitted `runId` addresses the one run of this `workflowId` that is still
        // open: "If the runId isn't specified, the WorkflowExecutionCancelRequested
        // event is recorded in the history of the current open workflow execution with
        // the specified workflowId in the domain."
        let Some(run_id) = domain.resolve_run_id(&input.workflow_id, input.run_id.as_deref())
        else {
            return Err(unknown().into());
        };
        let key = (input.workflow_id.clone(), run_id);
        let Some(execution) = domain.executions.get_mut(&key) else {
            return Err(unknown().into());
        };
        // Only a "currently running workflow execution" can be asked to cancel; a run
        // that has already closed is no longer available for this operation.
        if !execution.is_open() {
            return Err(unknown().into());
        }

        // A cancellation request is a LATCH on the run: once recorded it stays
        // requested until the decider acts, and a run carries at most one of them. So a
        // repeated request is accepted — the state it asks for is already recorded —
        // and appends nothing. That also keeps the stored history from growing once per
        // call for as long as a run stays open, the same memory bound
        // [`crate::state::MAX_DOMAINS`] and [`crate::state::MAX_EXECUTIONS_PER_DOMAIN`]
        // exist for.
        if execution.cancel_requested {
            return Ok(RequestCancelWorkflowExecutionOutput);
        }

        // Amazon SWF Quotas bounds one run's history, and a run that reaches that bound
        // is TERMINATED by the service rather than grown further. This request records
        // the cancel-requested event and the decision task that carries it to a decider,
        // so a run without room for both is closed instead — and a closed run is out of
        // this operation's scope, exactly as above.
        if !execution.has_history_room_for(2) {
            domain.close_run(
                &key,
                crate::state::RunClose::event_limit_exceeded(),
                requested_at,
            );
            return Err(unknown().into());
        }
        let Some(execution) = domain.executions.get_mut(&key) else {
            return Err(unknown().into());
        };

        // The run stays OPEN and keeps its close status absent: this call only
        // "logically requests the cancellation of the workflow execution as a whole. It
        // is up to the decider to take appropriate actions when it receives an execution
        // history with this event." Recording the event and scheduling the decision task
        // that carries it to a decider is the shared state path every requester of a
        // cancellation takes.
        execution.request_cancel(
            WorkflowExecutionCancelRequestedEventAttributes {
                // Both members describe a request made by ANOTHER execution's
                // `RequestCancelExternalWorkflowExecution` decision; this request
                // came straight from the caller, so there is no external execution
                // and no initiating event. `externalInitiatedEventId` models a
                // default of 0, which is that "no initiating event" value.
                external_workflow_execution: None,
                external_initiated_event_id: 0,
                // `cause` is set only when SWF generated the request itself ("If
                // set, indicates that the request to cancel the workflow execution
                // was automatically generated"), its one modelled value being
                // CHILD_POLICY_APPLIED. This call is the caller's own.
                cause: None,
            },
            requested_at,
        );

        Ok(RequestCancelWorkflowExecutionOutput)
    }
}
