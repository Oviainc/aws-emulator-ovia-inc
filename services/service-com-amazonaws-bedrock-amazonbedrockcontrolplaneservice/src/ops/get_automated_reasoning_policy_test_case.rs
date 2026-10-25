// SPDX-License-Identifier: CC0-1.0

//! @op GetAutomatedReasoningPolicyTestCase
//!
//! Reports one test of an Automated Reasoning policy out of [`crate::state`]: the test
//! `CreateAutomatedReasoningPolicyTestCase` stored under the policy the request addresses,
//! with the guard content, query content, expected check result, confidence threshold and
//! instants that create wrote. A pure READ — it writes nothing, so what one caller reads
//! is what the next one reads.
//!
//! A policy ARN this account holds no policy under, and a `testCaseId` that policy holds no
//! test under, are both refused with `ResourceNotFoundException`: a test belongs to the
//! policy it was created under, so an id minted under another policy names no test of this
//! one.
//!
//! Implemented here together with the create that mints the test — the create's own issue
//! is closed by proving the round trip through this read.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetAutomatedReasoningPolicyTestCase`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Retrieves details about a specific Automated Reasoning policy test.
//!
//! ## Input
//!
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy that
//!   contains the test.
//! * `testCaseId` (AutomatedReasoningPolicyTestCaseId, len ..12, pattern ^[0-9A-Z]{12}$)
//!   **required** — The unique identifier of the test to retrieve.
//!
//! ## Output
//!
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the policy that contains the test.
//! * `testCase` (AutomatedReasoningPolicyTestCase) **required** — The test details including
//!   the content, query, expected result, and metadata.
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
// `GetAutomatedReasoningPolicyTestCaseError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetAutomatedReasoningPolicyTestCaseError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#GetAutomatedReasoningPolicyTestCase")]
#[derive(Debug)]
pub enum GetAutomatedReasoningPolicyTestCaseErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_automated_reasoning_policy_test_case::GetAutomatedReasoningPolicyTestCaseError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetAutomatedReasoningPolicyTestCaseError = crate::scaffolded::Scaffolded<GetAutomatedReasoningPolicyTestCaseErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetAutomatedReasoningPolicyTestCaseErrorModeled> for GetAutomatedReasoningPolicyTestCaseError {
    fn from(modeled: GetAutomatedReasoningPolicyTestCaseErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetAutomatedReasoningPolicyTestCase` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetAutomatedReasoningPolicyTestCase<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetAutomatedReasoningPolicyTestCase<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetAutomatedReasoningPolicyTestCase<Eng>
{
    type Input = GetAutomatedReasoningPolicyTestCaseInput;
    type Output = GetAutomatedReasoningPolicyTestCaseOutput;
    type Error = GetAutomatedReasoningPolicyTestCaseError;

    const NAME: &'static str = "GetAutomatedReasoningPolicyTestCase";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_AUTOMATED_REASONING_POLICY_TEST_CASE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_AUTOMATED_REASONING_POLICY_TEST_CASE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                GetAutomatedReasoningPolicyTestCaseErrorModeled::InternalServerException(
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

        // The policy ARN is matched as the caller spelled it, exactly as a read of the
        // policy itself matches it; the test is then looked up under that policy, so an id
        // minted under another policy names no test here.
        if store.automated_reasoning_policy(&input.policy_arn).is_none() {
            return Err(not_found(&format!(
                "no automated reasoning policy {} exists in this account",
                input.policy_arn
            )));
        }
        let Some(test_case) =
            store.automated_reasoning_policy_test_case(&input.policy_arn, &input.test_case_id)
        else {
            return Err(not_found(&format!(
                "no test {} exists for automated reasoning policy {}",
                input.test_case_id, input.policy_arn
            )));
        };

        Ok(GetAutomatedReasoningPolicyTestCaseOutput {
            test_case: crate::state::automated_reasoning_policy_test_case(test_case),
            policy_arn: input.policy_arn,
        })
    }
}

/// The modelled 404 this operation answers for a policy or a test this account holds none
/// of — one wording for both, because the test is addressed by the PAIR.
fn not_found(message: &str) -> GetAutomatedReasoningPolicyTestCaseError {
    GetAutomatedReasoningPolicyTestCaseErrorModeled::ResourceNotFoundException(
        ResourceNotFoundException {
            message: Some(message.to_string()),
        },
    )
    .into()
}
