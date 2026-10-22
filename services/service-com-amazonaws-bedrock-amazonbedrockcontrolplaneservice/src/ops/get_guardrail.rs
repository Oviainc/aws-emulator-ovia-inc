// SPDX-License-Identifier: CC0-1.0

//! @op GetGuardrail
//!
//! Reads the stored guardrail out of [`crate::state`] and reports the revision the
//! request names — the DRAFT when `guardrailVersion` is absent or `DRAFT`, otherwise the
//! numbered snapshot. A `guardrailIdentifier` may be the bare id or the full ARN, as its
//! pattern admits both. An identifier or version this store does not hold is
//! `ResourceNotFoundException`.
//!
//! This operation is `@readonly`: it reports the stored revision's status and writes
//! nothing.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetGuardrail`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Gets details about a guardrail. If you don't specify a version, the response returns
//! details for the DRAFT version.
//!
//! ## Input
//!
//! * `guardrailIdentifier` (GuardrailIdentifier, len ..2048, pattern
//!   ^(([a-z0-9]+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+))$)
//!   **required** — The unique identifier of the guardrail for which to get details. This can
//!   be an ID or the ARN.
//! * `guardrailVersion` (GuardrailVersion, pattern ^(([1-9][0-9]{0,7})|(DRAFT))$) — The
//!   version of the guardrail for which to get details. If you don't specify a version, the
//!   response returns details for the DRAFT version.
//!
//! ## Output
//!
//! * `automatedReasoningPolicy` (GuardrailAutomatedReasoningPolicy) — The current Automated
//!   Reasoning policy configuration for the guardrail, if any is configured.
//! * `blockedInputMessaging` (GuardrailBlockedMessaging, len 1..500) **required** — The
//!   message that the guardrail returns when it blocks a prompt.
//! * `blockedOutputsMessaging` (GuardrailBlockedMessaging, len 1..500) **required** — The
//!   message that the guardrail returns when it blocks a model response.
//! * `contentPolicy` (GuardrailContentPolicy) — The content policy that was configured for
//!   the guardrail.
//! * `contextualGroundingPolicy` (GuardrailContextualGroundingPolicy) — The contextual
//!   grounding policy used in the guardrail.
//! * `createdAt` (Timestamp) **required** — The date and time at which the guardrail was
//!   created.
//! * `crossRegionDetails` (GuardrailCrossRegionDetails) — Details about the system-defined
//!   guardrail profile that you're using with your guardrail, including the guardrail profile
//!   ID and Amazon Resource Name (ARN).
//! * `description` (GuardrailDescription, len 1..200) — The description of the guardrail.
//! * `failureRecommendations` (GuardrailFailureRecommendations, len ..100) — Appears if the
//!   status of the guardrail is FAILED. A list of recommendations to carry out before
//!   retrying the request.
//! * `guardrailArn` (GuardrailArn, len ..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+$) **required** —
//!   The ARN of the guardrail.
//! * `guardrailId` (GuardrailId, len ..64, pattern ^[a-z0-9]+$) **required** — The unique
//!   identifier of the guardrail.
//! * `kmsKeyArn` (KmsKeyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$) — The ARN of the
//!   KMS key that encrypts the guardrail.
//! * `name` (GuardrailName, len 1..50, pattern ^[0-9a-zA-Z-_]+$) **required** — The name of
//!   the guardrail.
//! * `sensitiveInformationPolicy` (GuardrailSensitiveInformationPolicy) — The sensitive
//!   information policy that was configured for the guardrail.
//! * `status` (GuardrailStatus, one of: CREATING | DELETING | FAILED | READY | UPDATING |
//!   VERSIONING) **required** — The status of the guardrail.
//! * `statusReasons` (GuardrailStatusReasons, len ..100) — Appears if the status is FAILED. A
//!   list of reasons for why the guardrail failed to be created, updated, versioned, or
//!   deleted.
//! * `topicPolicy` (GuardrailTopicPolicy) — The topic policy that was configured for the
//!   guardrail.
//! * `updatedAt` (Timestamp) **required** — The date and time at which the guardrail was
//!   updated.
//! * `version` (GuardrailVersion, pattern ^(([1-9][0-9]{0,7})|(DRAFT))$) **required** — The
//!   version of the guardrail.
//! * `wordPolicy` (GuardrailWordPolicy) — The word policy that was configured for the
//!   guardrail.
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
// `GetGuardrailError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetGuardrailError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#GetGuardrail")]
#[derive(Debug)]
pub enum GetGuardrailErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_guardrail::GetGuardrailError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetGuardrailError = crate::scaffolded::Scaffolded<GetGuardrailErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetGuardrailErrorModeled> for GetGuardrailError {
    fn from(modeled: GetGuardrailErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetGuardrail` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetGuardrail<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetGuardrail<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetGuardrail<Eng>
{
    type Input = GetGuardrailInput;
    type Output = GetGuardrailOutput;
    type Error = GetGuardrailError;

    const NAME: &'static str = "GetGuardrail";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_GUARDRAIL_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_GUARDRAIL_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetGuardrailErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("guardrail state is not available on this engine".to_string()),
                },
            )
            .into());
        };
        let store = state.lock();

        let not_found = || {
            GetGuardrailErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(format!(
                    "no guardrail {} exists in this account",
                    input.guardrail_identifier
                )),
            })
        };

        let Some(guardrail) = store.guardrail(&input.guardrail_identifier) else {
            return Err(not_found().into());
        };
        let Some(revision) = guardrail.revision(input.guardrail_version.as_deref()) else {
            return Err(not_found().into());
        };

        Ok(GetGuardrailOutput {
            name: guardrail.name.clone(),
            description: revision.description.clone(),
            guardrail_id: guardrail.id.clone(),
            guardrail_arn: guardrail.arn.clone(),
            version: revision.version.clone(),
            status: revision.status.to_string(),
            topic_policy: revision.topic_policy.clone(),
            content_policy: revision.content_policy.clone(),
            word_policy: revision.word_policy.clone(),
            sensitive_information_policy: revision.sensitive_information_policy.clone(),
            contextual_grounding_policy: revision.contextual_grounding_policy.clone(),
            automated_reasoning_policy: revision.automated_reasoning_policy.clone(),
            cross_region_details: revision.cross_region_details.clone(),
            created_at: crate::state::date_time(revision.created_at),
            updated_at: crate::state::date_time(revision.updated_at),
            // `statusReasons` and `failureRecommendations` diagnose a FAILED guardrail,
            // and are the EMPTY list — present, not absent — for every other status:
            // both are modelled lists whose `@length` admits zero entries, so `[]` is
            // the wire form of "nothing to report". No revision this emulator mints
            // reaches FAILED, so both are read back empty from the stored revision.
            status_reasons: Some(revision.status_reasons.clone()),
            failure_recommendations: Some(revision.failure_recommendations.clone()),
            blocked_input_messaging: revision.blocked_input_messaging.clone(),
            blocked_outputs_messaging: revision.blocked_outputs_messaging.clone(),
            kms_key_arn: guardrail.kms_key_arn.clone(),
        })
    }
}
