// SPDX-License-Identifier: CC0-1.0

//! @op DeleteAutomatedReasoningPolicyBuildWorkflow
//!
//! Removes one stored Automated Reasoning policy build workflow from [`crate::state`],
//! addressed by the `(policyArn, buildWorkflowId)` pair every operation on a build workflow
//! addresses it by, once `lastUpdatedAt` names the write that workflow currently holds — so
//! a later read, cancel or listing of that policy's build workflows no longer finds it.
//!
//! The workflow's own ARTIFACTS go with it, because each of them is addressed and reported
//! THROUGH the workflow and nothing could read one that outlived it: the test runs a test
//! workflow scheduled against it, the token of the test workflow that scheduled them, and
//! the `clientRequestToken` the start that minted it was replayable under.
//!
//! A pair naming no stored workflow is refused with `ResourceNotFoundException`, carrying
//! the wording this resource is refused with wherever the request came from
//! ([`crate::state::build_workflow_not_found_message`]). The operation documents no status
//! precondition, so a workflow is removed whatever status it holds, and the policy it was
//! building is free for another build once it is gone.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteAutomatedReasoningPolicyBuildWorkflow`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Deletes an Automated Reasoning policy build workflow and its associated artifacts. This
//! permanently removes the workflow history and any generated assets.
//!
//! ## Input
//!
//! * `buildWorkflowId` (AutomatedReasoningPolicyBuildWorkflowId, len ..36, pattern
//!   ^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$) **required**
//!   — The unique identifier of the build workflow to delete.
//! * `lastUpdatedAt` (Timestamp) **required** — The timestamp when the build workflow was
//!   last updated. This is used for optimistic concurrency control to prevent accidental
//!   deletion of workflows that have been modified.
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy whose
//!   build workflow you want to delete.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `ConflictException` (HTTP 400) — Error occurred because of a conflict while performing
//!   an operation.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceInUseException` (HTTP 400) — Thrown when attempting to delete or modify a
//!   resource that is currently being used by other resources or operations. For example,
//!   trying to delete an Automated Reasoning policy that is referenced by an active
//!   guardrail.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

/// The write this operation performs, as the conflict message names it
/// ([`crate::state::automated_reasoning_policy_build_workflow_conflict_message`]).
const DELETING: &str = "deleting";

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteAutomatedReasoningPolicyBuildWorkflowError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteAutomatedReasoningPolicyBuildWorkflowError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#DeleteAutomatedReasoningPolicyBuildWorkflow")]
#[derive(Debug)]
pub enum DeleteAutomatedReasoningPolicyBuildWorkflowErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_automated_reasoning_policy_build_workflow::DeleteAutomatedReasoningPolicyBuildWorkflowError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteAutomatedReasoningPolicyBuildWorkflowError = crate::scaffolded::Scaffolded<DeleteAutomatedReasoningPolicyBuildWorkflowErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteAutomatedReasoningPolicyBuildWorkflowErrorModeled> for DeleteAutomatedReasoningPolicyBuildWorkflowError {
    fn from(modeled: DeleteAutomatedReasoningPolicyBuildWorkflowErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteAutomatedReasoningPolicyBuildWorkflow` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteAutomatedReasoningPolicyBuildWorkflow<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteAutomatedReasoningPolicyBuildWorkflow<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteAutomatedReasoningPolicyBuildWorkflow<Eng>
{
    type Input = DeleteAutomatedReasoningPolicyBuildWorkflowInput;
    type Output = DeleteAutomatedReasoningPolicyBuildWorkflowOutput;
    type Error = DeleteAutomatedReasoningPolicyBuildWorkflowError;

    const NAME: &'static str = "DeleteAutomatedReasoningPolicyBuildWorkflow";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_AUTOMATED_REASONING_POLICY_BUILD_WORKFLOW_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_AUTOMATED_REASONING_POLICY_BUILD_WORKFLOW_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 202u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(server_error());
        };
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        // The pair selects, exactly as a read of the workflow selects: a workflow is only
        // ever addressed under the policy ARN it was started for, so the same id under
        // another policy names no workflow to delete. The lookup settles first, so the
        // write this call holds the token against is the one the workflow holds at `now`.
        let Some(workflow) = store.automated_reasoning_policy_build_workflow(
            &input.policy_arn,
            &input.build_workflow_id,
            now,
        ) else {
            return Err(not_found(
                crate::state::build_workflow_not_found_message().to_string(),
            ));
        };

        // `lastUpdatedAt` is the optimistic concurrency token the member documents: the
        // delete goes through only while it names the write the workflow currently holds,
        // so a delete issued against a workflow that has moved on since the caller read it
        // is refused rather than dropping that workflow's history with it.
        if !crate::state::names_write_at(&input.last_updated_at, workflow.updated_at) {
            return Err(conflict(
                crate::state::automated_reasoning_policy_build_workflow_conflict_message(
                    DELETING,
                    &input.build_workflow_id,
                    &input.policy_arn,
                ),
            ));
        }

        // The pair was resolved under this same lock a few lines above, so the entry is
        // there to take and no caller can have removed it in between.
        store
            .automated_reasoning_policy_build_workflows
            .remove(&(input.policy_arn.clone(), input.build_workflow_id.clone()));
        // "its associated artifacts": a test run is addressed and reported THROUGH the
        // build workflow it tested, and the test workflow token is held per build workflow,
        // so both go with the workflow rather than staying behind as state no caller can
        // reach. No replayed start token may hand a later caller the workflow that is now
        // gone either.
        store
            .automated_reasoning_policy_test_runs
            .retain(|(policy_arn, build_workflow_id, _), _| {
                !(*policy_arn == input.policy_arn
                    && *build_workflow_id == input.build_workflow_id)
            });
        store
            .automated_reasoning_policy_test_workflow_tokens
            .retain(|(policy_arn, build_workflow_id), _| {
                !(*policy_arn == input.policy_arn
                    && *build_workflow_id == input.build_workflow_id)
            });
        store
            .automated_reasoning_policy_build_workflow_tokens
            .retain(|(policy_arn, _), started| {
                !(*policy_arn == input.policy_arn && *started == input.build_workflow_id)
            });

        Ok(DeleteAutomatedReasoningPolicyBuildWorkflowOutput {})
    }
}

/// The modelled 404 this operation answers for a pair this store holds no build workflow
/// under — a workflow is addressed by the PAIR, so either half missing is "the resource was
/// not found".
fn not_found(message: String) -> DeleteAutomatedReasoningPolicyBuildWorkflowError {
    DeleteAutomatedReasoningPolicyBuildWorkflowErrorModeled::ResourceNotFoundException(
        ResourceNotFoundException {
            message: Some(message),
        },
    )
    .into()
}

/// The modelled 400 this operation answers when `lastUpdatedAt` does not name the write the
/// addressed workflow holds.
fn conflict(message: String) -> DeleteAutomatedReasoningPolicyBuildWorkflowError {
    DeleteAutomatedReasoningPolicyBuildWorkflowErrorModeled::ConflictException(
        ConflictException {
            message: Some(message),
        },
    )
    .into()
}

/// The modelled 500 this operation answers when the engine carries no store for this
/// service — never a fresh engine, which would drop the delete on return.
fn server_error() -> DeleteAutomatedReasoningPolicyBuildWorkflowError {
    DeleteAutomatedReasoningPolicyBuildWorkflowErrorModeled::InternalServerException(
        InternalServerException {
            message: Some(
                "automated reasoning policy state is not available on this engine".to_string(),
            ),
        },
    )
    .into()
}
