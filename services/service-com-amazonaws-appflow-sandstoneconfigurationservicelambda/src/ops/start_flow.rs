// SPDX-License-Identifier: CC0-1.0

//! @op StartFlow
//! Runs a stored flow: appends a run to that flow's history and answers with the
//! identifier of the run it recorded — which `DescribeFlowExecutionRecords` then
//! returns. The stored DEFINITION is untouched; only the run history grows.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `StartFlow`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda/smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json")
//! ```
//!
//! Activates an existing flow. For on-demand flows, this operation runs the flow
//! immediately. For schedule and event-triggered flows, this operation activates the flow.
//!
//! ## Input
//!
//! * `clientToken` (ClientToken, len 1..256, pattern ^[ -~]+$, @idempotencyToken) — The
//!   clientToken parameter is an idempotency token. It ensures that your StartFlow request
//!   completes only once. You choose the value to pass. For example, if you don't receive a
//!   response from your request, you can safely retry the request with the same clientToken
//!   parameter value. If you omit a clientToken value, the Amazon Web Services SDK that you
//!   are using inserts a value for you. This way, the SDK can safely retry requests multiple
//!   times after a network error. You must provide your own value for other use cases. If you
//!   specify input parameters that differ from your first request, an error occurs for flows
//!   that run on a schedule or based on an event. However, the error doesn't occur for …
//! * `flowName` (FlowName, len 0..256, pattern ^[a-zA-Z0-9][\w!@#.-]+$) **required** — The
//!   specified name of the flow. Spaces are not allowed. Use underscores (_) or hyphens (-)
//!   only.
//!
//! ## Output
//!
//! * `executionId` (ExecutionId, len 0..256, pattern ^\S+$) — Returns the internal execution
//!   ID of an on-demand flow when the flow is started. For scheduled or event-triggered
//!   flows, this value is null.
//! * `flowArn` (FlowArn, len 0..512, pattern ^arn:aws:appflow:.*:[0-9]+:) — The flow's Amazon
//!   Resource Name (ARN).
//! * `flowStatus` (FlowStatus, one of: Active | Deleted | Deprecated | Draft | Errored |
//!   Suspended) — Indicates the current status of the flow.
//!
//! ## Errors
//!
//! * `ConflictException` (HTTP 409) — There was a conflict when processing the request (for
//!   example, a flow with the given name already exists within the account. Check for
//!   conflicting resource names and try again.
//! * `InternalServerException` (HTTP 500) — An internal service error occurred during the
//!   processing of your request. Try again later.
//! * `ResourceNotFoundException` (HTTP 404) — The resource specified in the request (such as
//!   the source or destination connector profile) is not found.
//! * `ServiceQuotaExceededException` (HTTP 402) — The request would cause a service quota
//!   (such as the number of flows) to be exceeded.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `StartFlowError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StartFlowError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.appflow#StartFlow")]
#[derive(Debug)]
pub enum StartFlowErrorModeled {
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_flow::StartFlowError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StartFlowError = crate::scaffolded::Scaffolded<StartFlowErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StartFlowErrorModeled> for StartFlowError {
    fn from(modeled: StartFlowErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartFlow` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StartFlow<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartFlow<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartFlow<Eng>
{
    type Input = StartFlowInput;
    type Output = StartFlowOutput;
    type Error = StartFlowError;

    const NAME: &'static str = "StartFlow";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_FLOW_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_FLOW_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            StartFlowErrorModeled::InternalServerException(InternalServerException {
                message: Some("the AppFlow emulator holds no state on this engine".to_string()),
            })
        })?;

        let now = ::std::time::SystemTime::now();

        let mut store = state.lock();
        let flow = store.flows.get_mut(&input.flow_name).ok_or_else(|| {
            StartFlowErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(format!(
                    "No flow named {} exists in this account",
                    input.flow_name
                )),
            })
        })?;
        flow.settle_due_executions(now);

        // A retried StartFlow carrying the same `@idempotencyToken` answers with the run
        // the FIRST call recorded, rather than starting a second one.
        if let Some(execution) = input
            .client_token
            .as_deref()
            .and_then(|token| flow.execution_for_client_token(token))
        {
            return Ok(StartFlowOutput {
                flow_arn: Some(flow.flow_arn.clone()),
                flow_status: Some(flow.flow_status.as_str().to_string()),
                execution_id: Some(execution.execution_id.clone()),
            });
        }

        if flow.has_running_execution() {
            return Err(StartFlowErrorModeled::ConflictException(ConflictException {
                message: Some(format!(
                    "Flow {} is already running and cannot be started again",
                    flow.flow_name
                )),
            })
            .into());
        }
        // Starting a schedule- or event-triggered flow ARMS its trigger, so one whose
        // trigger is already armed has nothing left to start.
        if !flow.is_on_demand() && flow.flow_status == crate::state::FlowStatus::Active {
            return Err(StartFlowErrorModeled::ConflictException(ConflictException {
                message: Some(format!(
                    "Flow {} is already activated and cannot be started again",
                    flow.flow_name
                )),
            })
            .into());
        }

        // Starting a flow does not re-define it, so nothing about the stored definition
        // moves here: `lastUpdatedAt`/`lastUpdatedBy` describe the definition, and only
        // the trigger and the run history below change.
        //
        // A flow that runs ITSELF is activated rather than run now, and reports no run:
        // `executionId` "[r]eturns the internal execution ID of an on-demand flow when
        // the flow is started. For scheduled or event-triggered flows, this value is
        // null" (`smithy_query(query="doc", shape="StartFlowResponse",
        // member="executionId")`). Only an on-demand flow starts a run here.
        if flow.activate_trigger() {
            return Ok(StartFlowOutput {
                flow_arn: Some(flow.flow_arn.clone()),
                flow_status: Some(flow.flow_status.as_str().to_string()),
                execution_id: None,
            });
        }

        let execution_id = crate::state::new_execution_id();
        flow.record_execution(crate::state::FlowExecution {
            execution_id: execution_id.clone(),
            client_token: input.client_token,
            execution_status: crate::state::ExecutionStatus::InProgress,
            execution_result: None,
            started_at: now,
            last_updated_at: now,
            // The terminal status this run is owed, which a later read materialises.
            settles_at: Some(now + crate::state::EXECUTION_SETTLES_AFTER),
            data_pull_start_time: None,
            data_pull_end_time: None,
        });

        Ok(StartFlowOutput {
            flow_arn: Some(flow.flow_arn.clone()),
            flow_status: Some(flow.flow_status.as_str().to_string()),
            execution_id: Some(execution_id),
        })
    }
}
