// SPDX-License-Identifier: CC0-1.0

//! @op GetAutomatedReasoningPolicyBuildWorkflow
//!
//! Reports the build workflow a `(policyArn, buildWorkflowId)` pair names out of
//! [`crate::state`]: the workflow type and the source document
//! `StartAutomatedReasoningPolicyBuildWorkflow` stored, the status it holds at the instant
//! of the read, and the instants it was started and last changed. A pair naming no stored
//! workflow of this account is refused with `ResourceNotFoundException`.
//!
//! A REPORTING operation (`@readonly`): the only write it makes is materialising the status
//! moves of the build's progression the clock had already made due
//! ([`crate::state::AutomatedReasoningPolicyBuildWorkflowRecord::settle`]), each of which was
//! owed at its own instant even if nobody had called.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetAutomatedReasoningPolicyBuildWorkflow`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Retrieves detailed information about an Automated Reasoning policy build workflow,
//! including its status, configuration, and metadata.
//!
//! ## Input
//!
//! * `buildWorkflowId` (AutomatedReasoningPolicyBuildWorkflowId, len ..36, pattern
//!   ^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$) **required**
//!   — The unique identifier of the build workflow to retrieve.
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy whose
//!   build workflow you want to retrieve.
//!
//! ## Output
//!
//! * `buildWorkflowId` (AutomatedReasoningPolicyBuildWorkflowId, len ..36, pattern
//!   ^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$) **required**
//!   — The unique identifier of the build workflow.
//! * `buildWorkflowType` (AutomatedReasoningPolicyBuildWorkflowType, one of:
//!   GENERATE_FIDELITY_REPORT | GENERATE_POLICY_SCENARIOS | IMPORT_POLICY | INGEST_CONTENT |
//!   ITERATIVELY_REFINE_POLICY | REFINE_POLICY | RESOLVE_POLICY_AMBIGUITIES) **required** —
//!   The type of build workflow being executed (e.g., DOCUMENT_INGESTION, POLICY_REPAIR).
//! * `createdAt` (Timestamp) **required** — The timestamp when the build workflow was
//!   created.
//! * `documentContentType` (AutomatedReasoningPolicyBuildDocumentContentType, one of: pdf |
//!   txt) — The content type of the source document (e.g., text/plain, application/pdf).
//! * `documentDescription` (AutomatedReasoningPolicyBuildDocumentDescription, len ..4000) — A
//!   detailed description of the document's content and how it should be used in the policy
//!   generation process.
//! * `documentName` (AutomatedReasoningPolicyBuildDocumentName, len ..256) — The name of the
//!   source document used in the build workflow.
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy.
//! * `status` (AutomatedReasoningPolicyBuildWorkflowStatus, one of: BUILDING | CANCELLED |
//!   CANCEL_REQUESTED | COMPLETED | FAILED | PREPROCESSING | SCHEDULED | TESTING)
//!   **required** — The current status of the build workflow (e.g., RUNNING, COMPLETED,
//!   FAILED, CANCELLED).
//! * `updatedAt` (Timestamp) **required** — The timestamp when the build workflow was last
//!   updated.
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
// `GetAutomatedReasoningPolicyBuildWorkflowError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetAutomatedReasoningPolicyBuildWorkflowError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#GetAutomatedReasoningPolicyBuildWorkflow")]
#[derive(Debug)]
pub enum GetAutomatedReasoningPolicyBuildWorkflowErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_automated_reasoning_policy_build_workflow::GetAutomatedReasoningPolicyBuildWorkflowError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetAutomatedReasoningPolicyBuildWorkflowError = crate::scaffolded::Scaffolded<GetAutomatedReasoningPolicyBuildWorkflowErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetAutomatedReasoningPolicyBuildWorkflowErrorModeled> for GetAutomatedReasoningPolicyBuildWorkflowError {
    fn from(modeled: GetAutomatedReasoningPolicyBuildWorkflowErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetAutomatedReasoningPolicyBuildWorkflow` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetAutomatedReasoningPolicyBuildWorkflow<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetAutomatedReasoningPolicyBuildWorkflow<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetAutomatedReasoningPolicyBuildWorkflow<Eng>
{
    type Input = GetAutomatedReasoningPolicyBuildWorkflowInput;
    type Output = GetAutomatedReasoningPolicyBuildWorkflowOutput;
    type Error = GetAutomatedReasoningPolicyBuildWorkflowError;

    const NAME: &'static str = "GetAutomatedReasoningPolicyBuildWorkflow";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_AUTOMATED_REASONING_POLICY_BUILD_WORKFLOW_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_AUTOMATED_REASONING_POLICY_BUILD_WORKFLOW_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                GetAutomatedReasoningPolicyBuildWorkflowErrorModeled::InternalServerException(
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

        // The pair selects: a workflow is only ever read under the policy ARN it was
        // started for, so the same id under another policy names no workflow.
        let Some(workflow) = store.automated_reasoning_policy_build_workflow(
            &input.policy_arn,
            &input.build_workflow_id,
            now,
        ) else {
            return Err(
                GetAutomatedReasoningPolicyBuildWorkflowErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(
                            crate::state::build_workflow_not_found_message().to_string(),
                        ),
                    },
                )
                .into(),
            );
        };

        Ok(GetAutomatedReasoningPolicyBuildWorkflowOutput {
            policy_arn: workflow.policy_arn.clone(),
            build_workflow_id: workflow.id.clone(),
            status: workflow.status.to_string(),
            build_workflow_type: workflow.workflow_type.clone(),
            document_name: workflow.document_name.clone(),
            document_content_type: workflow.document_content_type.clone(),
            document_description: workflow.document_description.clone(),
            created_at: crate::state::date_time(workflow.created_at),
            updated_at: crate::state::date_time(workflow.updated_at),
        })
    }
}
