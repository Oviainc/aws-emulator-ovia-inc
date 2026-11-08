// SPDX-License-Identifier: CC0-1.0

//! @op DescribeWorkflowExecution
//!
//! A pure read: it reports the ONE stored execution record addressed by
//! `(domain, workflowId, runId)` — its info, the configuration resolved at start time
//! and its live open counts — and writes nothing.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeWorkflowExecution`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-swf-simpleworkflowservice/smithy-model/com-amazonaws-swf-simpleworkflowservice.json")
//! ```
//!
//! Returns information about the specified workflow execution including its type and some
//! statistics. This operation is eventually consistent. The results are best effort and may
//! not exactly reflect recent updates and changes. Access Control You can use IAM policies
//! to control this action's access to Amazon SWF resources as follows: Use a Resource
//! element with the domain name to limit the action to only specified domains. Use an
//! Action element to allow or deny permission to call this action. You cannot use an IAM
//! policy to constrain this action's parameters. If the caller doesn't have sufficient
//! permissions to invoke the action, or the parameter values fall outside the specified
//! constraints, the action fails. The associated event attribute's cause parameter is set
//! to OPERATION_NOT_PERMITTED. For details and example IAM policies, see Using IAM to
//! Manage Access to Amazon SWF Workflows in the Amazon SWF Developer Guide.
//!
//! ## Input
//!
//! * `domain` (DomainName, len 1..256) **required** — The name of the domain containing the
//!   workflow execution.
//! * `execution` (WorkflowExecution) **required** — The workflow execution to describe.
//!
//! ## Output
//!
//! * `executionConfiguration` (WorkflowExecutionConfiguration) **required** — The
//!   configuration settings for this workflow execution including timeout values, tasklist
//!   etc.
//! * `executionInfo` (WorkflowExecutionInfo) **required** — Information about the workflow
//!   execution.
//! * `latestActivityTaskTimestamp` (Timestamp) — The time when the last activity task was
//!   scheduled for this workflow execution. You can use this information to determine if the
//!   workflow has not made progress for an unusually long period of time and might require a
//!   corrective action.
//! * `latestExecutionContext` (Data, len 0..32768) — The latest executionContext provided by
//!   the decider for this workflow execution. A decider can provide an executionContext (a
//!   free-form string) when closing a decision task using RespondDecisionTaskCompleted.
//! * `openCounts` (WorkflowExecutionOpenCounts) **required** — The number of tasks for this
//!   workflow execution. This includes open and closed tasks of all types.
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
// `DescribeWorkflowExecutionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-swf-simpleworkflowservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeWorkflowExecutionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.swf#DescribeWorkflowExecution")]
#[derive(Debug)]
pub enum DescribeWorkflowExecutionErrorModeled {
    OperationNotPermittedFault(OperationNotPermittedFault),
    UnknownResourceFault(UnknownResourceFault),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_workflow_execution::DescribeWorkflowExecutionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeWorkflowExecutionError = crate::scaffolded::Scaffolded<DescribeWorkflowExecutionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeWorkflowExecutionErrorModeled> for DescribeWorkflowExecutionError {
    fn from(modeled: DescribeWorkflowExecutionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeWorkflowExecution` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeWorkflowExecution<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeWorkflowExecution<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeWorkflowExecution<Eng>
{
    type Input = DescribeWorkflowExecutionInput;
    type Output = DescribeWorkflowExecutionOutput;
    type Error = DescribeWorkflowExecutionError;

    const NAME: &'static str = "DescribeWorkflowExecution";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_WORKFLOW_EXECUTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_WORKFLOW_EXECUTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeWorkflowExecutionErrorModeled::OperationNotPermittedFault(
                OperationNotPermittedFault {
                    message: Some("the SWF domain store is unavailable".to_string()),
                },
            )
        })?;

        let swept_at = crate::state::now();
        let mut guard = state.lock();
        // The description is built only after the service's own start-to-close reaping
        // is materialised, so a run whose budget the clock already spent reports
        // `executionStatus=CLOSED` with `closeStatus=TIMED_OUT` instead of staying OPEN
        // forever.
        guard.materialise_due_timeouts_in(&input.domain, swept_at);
        // An `UnknownResourceFault` message names the resource the REQUEST asked for.
        // This operation addresses one execution, so the execution is what the fault
        // names — in the `WorkflowExecution=[workflowId=.., runId=..]` form every
        // execution-scoped operation of this service uses — whether the lookup missed
        // because the domain holds no such run or because this service holds no such
        // domain. The domain the lookup was scoped to is never part of that clause.
        let unknown_execution = || {
            DescribeWorkflowExecutionErrorModeled::UnknownResourceFault(UnknownResourceFault {
                message: Some(crate::state::unknown_execution_message(
                    &input.execution.workflow_id,
                    &input.execution.run_id,
                )),
            })
        };
        let domain = guard
            .domains
            .get(&input.domain)
            .ok_or_else(unknown_execution)?;
        let execution = domain
            .executions
            .get(&(
                input.execution.workflow_id.clone(),
                input.execution.run_id.clone(),
            ))
            .ok_or_else(unknown_execution)?;

        Ok(DescribeWorkflowExecutionOutput {
            execution_info: execution.info(),
            execution_configuration: execution.effective_config.configuration(),
            open_counts: execution.open_counts(),
            latest_activity_task_timestamp: execution.latest_activity_task_timestamp,
            latest_execution_context: execution.latest_execution_context.clone(),
        })
    }
}
