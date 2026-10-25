// SPDX-License-Identifier: CC0-1.0

//! @op GetAutomatedReasoningPolicy
//!
//! Reports the Automated Reasoning policy a `policyArn` names, read out of
//! [`crate::state`]: the identity `CreateAutomatedReasoningPolicy` minted, the description
//! and KMS key it was created with, the hash of its definition, and the instants it was
//! created and last written. An ARN this account holds no policy under is answered with
//! 404 `ResourceNotFoundException`.
//!
//! A pure READ: nothing about a policy is owed a change by the clock, so this writes
//! nothing.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetAutomatedReasoningPolicy`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Retrieves details about an Automated Reasoning policy or policy version. Returns
//! information including the policy definition, metadata, and timestamps.
//!
//! ## Input
//!
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy to
//!   retrieve. Can be either the unversioned ARN for the draft policy or an ARN for a
//!   specific policy version.
//!
//! ## Output
//!
//! * `createdAt` (Timestamp) — The timestamp when the policy was created.
//! * `definitionHash` (AutomatedReasoningPolicyHash, len 128..128, pattern ^[0-9a-z]{128}$)
//!   **required** — The hash of the policy definition used as a concurrency token.
//! * `description` (AutomatedReasoningPolicyDescription, len ..1024, pattern ^[\s\S]+$) — The
//!   description of the policy.
//! * `kmsKeyArn` (KmsKeyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$) — The Amazon
//!   Resource Name (ARN) of the KMS key used to encrypt the automated reasoning policy and
//!   its associated artifacts. If a KMS key is not provided during the initial
//!   CreateAutomatedReasoningPolicyRequest, the kmsKeyArn won't be included in the
//!   GetAutomatedReasoningPolicyResponse.
//! * `name` (AutomatedReasoningPolicyName, len 1..256, pattern ^[0-9a-zA-Z-_ ]+$)
//!   **required** — The name of the policy.
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the policy.
//! * `policyId` (AutomatedReasoningPolicyId, len 1..2048, pattern ^[a-z0-9]{12}$)
//!   **required** — The unique identifier of the policy.
//! * `updatedAt` (Timestamp) **required** — The timestamp when the policy was last updated.
//! * `version` (AutomatedReasoningPolicyVersion, len ..12, pattern ^([1-9][0-9]{0,11})$)
//!   **required** — The version of the policy.
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
// `GetAutomatedReasoningPolicyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetAutomatedReasoningPolicyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#GetAutomatedReasoningPolicy")]
#[derive(Debug)]
pub enum GetAutomatedReasoningPolicyErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_automated_reasoning_policy::GetAutomatedReasoningPolicyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetAutomatedReasoningPolicyError = crate::scaffolded::Scaffolded<GetAutomatedReasoningPolicyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetAutomatedReasoningPolicyErrorModeled> for GetAutomatedReasoningPolicyError {
    fn from(modeled: GetAutomatedReasoningPolicyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetAutomatedReasoningPolicy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetAutomatedReasoningPolicy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetAutomatedReasoningPolicy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetAutomatedReasoningPolicy<Eng>
{
    type Input = GetAutomatedReasoningPolicyInput;
    type Output = GetAutomatedReasoningPolicyOutput;
    type Error = GetAutomatedReasoningPolicyError;

    const NAME: &'static str = "GetAutomatedReasoningPolicy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_AUTOMATED_REASONING_POLICY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_AUTOMATED_REASONING_POLICY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                GetAutomatedReasoningPolicyErrorModeled::InternalServerException(
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
        let store = state.lock();

        // The ARN is matched as the caller spelled it, so an ARN of another region, and a
        // versioned ARN of a version nothing has minted, both name no policy this account
        // holds.
        let Some(policy) = store.automated_reasoning_policy(&input.policy_arn) else {
            return Err(
                GetAutomatedReasoningPolicyErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(format!(
                            "no automated reasoning policy {} exists in this account",
                            input.policy_arn
                        )),
                    },
                )
                .into(),
            );
        };

        Ok(GetAutomatedReasoningPolicyOutput {
            policy_arn: policy.arn.clone(),
            name: policy.name.clone(),
            // The unversioned ARN names the policy's working version, which the create
            // documents as always DRAFT.
            version: crate::state::DRAFT_VERSION.to_string(),
            policy_id: policy.id.clone(),
            description: policy.description.clone(),
            definition_hash: policy.definition_hash.clone(),
            kms_key_arn: policy.kms_key_arn.clone(),
            created_at: Some(crate::state::date_time(policy.created_at)),
            updated_at: crate::state::date_time(policy.updated_at),
        })
    }
}
