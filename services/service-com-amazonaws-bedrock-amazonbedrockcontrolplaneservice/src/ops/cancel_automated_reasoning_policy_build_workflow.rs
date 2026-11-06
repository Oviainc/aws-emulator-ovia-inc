// SPDX-License-Identifier: CC0-1.0

//! @op CancelAutomatedReasoningPolicyBuildWorkflow
//!
//! Asks a stored Automated Reasoning policy build workflow to stop, and PERSISTS that ask
//! in [`crate::state`]: a workflow that is still working moves to `CANCEL_REQUESTED`, is
//! owed neither the rest of its build's progression nor its completion any more, and the
//! clock owes it `CANCELLED`
//! ([`crate::state::BUILD_WORKFLOW_CANCELLED_AFTER`]) — so a caller reading straight after
//! the cancel still sees the status this call recorded, and
//! `GetAutomatedReasoningPolicyBuildWorkflow` reports exactly what this call wrote.
//!
//! A workflow that is already winding down, or has reached a terminal status of its own,
//! has nothing left to stop: the call changes nothing and is accepted, so one cancel and
//! two are indistinguishable (the operation carries no output members and is modelled
//! `@idempotent`). A pair naming no stored workflow is refused with
//! `ResourceNotFoundException`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CancelAutomatedReasoningPolicyBuildWorkflow`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Cancels a running Automated Reasoning policy build workflow. This stops the policy
//! generation process and prevents further processing of the source documents.
//!
//! ## Input
//!
//! * `buildWorkflowId` (AutomatedReasoningPolicyBuildWorkflowId, len ..36, pattern
//!   ^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$) **required**
//!   — The unique identifier of the build workflow to cancel. You can get this ID from the
//!   StartAutomatedReasoningPolicyBuildWorkflow response or by listing build workflows.
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy whose
//!   build workflow you want to cancel.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CancelAutomatedReasoningPolicyBuildWorkflowError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CancelAutomatedReasoningPolicyBuildWorkflowError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#CancelAutomatedReasoningPolicyBuildWorkflow")]
#[derive(Debug)]
pub enum CancelAutomatedReasoningPolicyBuildWorkflowErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::cancel_automated_reasoning_policy_build_workflow::CancelAutomatedReasoningPolicyBuildWorkflowError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CancelAutomatedReasoningPolicyBuildWorkflowError = crate::scaffolded::Scaffolded<CancelAutomatedReasoningPolicyBuildWorkflowErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CancelAutomatedReasoningPolicyBuildWorkflowErrorModeled> for CancelAutomatedReasoningPolicyBuildWorkflowError {
    fn from(modeled: CancelAutomatedReasoningPolicyBuildWorkflowErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CancelAutomatedReasoningPolicyBuildWorkflow` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CancelAutomatedReasoningPolicyBuildWorkflow<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CancelAutomatedReasoningPolicyBuildWorkflow<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CancelAutomatedReasoningPolicyBuildWorkflow<Eng>
{
    type Input = CancelAutomatedReasoningPolicyBuildWorkflowInput;
    type Output = CancelAutomatedReasoningPolicyBuildWorkflowOutput;
    type Error = CancelAutomatedReasoningPolicyBuildWorkflowError;

    const NAME: &'static str = "CancelAutomatedReasoningPolicyBuildWorkflow";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CANCEL_AUTOMATED_REASONING_POLICY_BUILD_WORKFLOW_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CANCEL_AUTOMATED_REASONING_POLICY_BUILD_WORKFLOW_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 202u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The engine is reachable from `&self` (constructor-injected); `ctx` carries
        // request metadata. Both are bound here so an implementer has them in scope.
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                CancelAutomatedReasoningPolicyBuildWorkflowErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "automated reasoning policy state is not available on this engine"
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        };
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        // The pair selects, exactly as `GetAutomatedReasoningPolicyBuildWorkflow` reads
        // it: a workflow is only ever addressed under the policy ARN it was started for,
        // so the same id under another policy names no workflow to cancel. The lookup
        // settles first, so the status this call decides on is the status the workflow
        // holds at `now`.
        let Some(workflow) = store.automated_reasoning_policy_build_workflow(
            &input.policy_arn,
            &input.build_workflow_id,
            now,
        ) else {
            return Err(
                CancelAutomatedReasoningPolicyBuildWorkflowErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(
                            crate::state::build_workflow_not_found_message().to_string(),
                        ),
                    },
                )
                .into(),
            );
        };

        // A workflow a cancel has already taken over is already going where this call asks
        // it to go, and one that reached a terminal status of its own has no work left to
        // stop: either way nothing changes and the call is accepted, so the workflow keeps
        // the status and the instants it already had.
        if workflow.is_cancelled() || !workflow.is_active() {
            return Ok(CancelAutomatedReasoningPolicyBuildWorkflowOutput {});
        }

        // Cancelling is asynchronous, so the workflow is reported `CANCEL_REQUESTED` from
        // here onwards and the clock owes it the terminal status.
        workflow.cancel(now);
        Ok(CancelAutomatedReasoningPolicyBuildWorkflowOutput {})
    }
}
