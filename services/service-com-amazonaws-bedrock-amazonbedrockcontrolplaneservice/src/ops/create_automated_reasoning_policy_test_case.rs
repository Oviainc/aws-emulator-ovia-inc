// SPDX-License-Identifier: CC0-1.0

//! @op CreateAutomatedReasoningPolicyTestCase
//!
//! Mints a test for an Automated Reasoning policy and PERSISTS it in [`crate::state`]: the
//! `testCaseId` it minted, the guard content and query content the request carried, the
//! expected check result and the confidence threshold, and the creation instant. The test
//! is stored under the policy the request addressed, so what this mints is exactly what
//! `GetAutomatedReasoningPolicyTestCase` finds under that policy.
//!
//! `guardContent` and `queryContent` are stored as the request carried them, character for
//! character: they are the sample the policy is validated against, so the read reports the
//! text that was written rather than any normalisation of it.
//!
//! A policy ARN this account holds no policy under is refused with
//! `ResourceNotFoundException`, a replay of the same `clientRequestToken` answers with the
//! test that token already minted (`@idempotencyToken`), and a policy already holding the
//! most tests one may hold is refused with `ServiceQuotaExceededException`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateAutomatedReasoningPolicyTestCase`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Creates a test for an Automated Reasoning policy. Tests validate that your policy works
//! as expected by providing sample inputs and expected outcomes. Use tests to verify policy
//! behavior before deploying to production.
//!
//! ## Input
//!
//! * `clientRequestToken` (IdempotencyToken, len 1..256, pattern
//!   ^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$, @idempotencyToken) — A unique,
//!   case-sensitive identifier to ensure that the operation completes no more than one time.
//!   If this token matches a previous request, Amazon Bedrock ignores the request, but does
//!   not return an error.
//! * `confidenceThreshold` (AutomatedReasoningCheckTranslationConfidence, range 0..1) — The
//!   minimum confidence level for logic validation. Content that meets the threshold is
//!   considered a high-confidence finding that can be validated.
//! * `expectedAggregatedFindingsResult` (AutomatedReasoningCheckResult, one of: IMPOSSIBLE |
//!   INVALID | NO_TRANSLATION | SATISFIABLE | TOO_COMPLEX | TRANSLATION_AMBIGUOUS | VALID)
//!   **required** — The expected result of the Automated Reasoning check. Valid values
//!   include: , TOO_COMPLEX, and NO_TRANSLATIONS. VALID - The claims are true. The claims are
//!   implied by the premises and the Automated Reasoning policy. Given the Automated
//!   Reasoning policy and premises, it is not possible for these claims to be false. In other
//!   words, there are no alternative answers that are true that contradict the claims.
//!   INVALID - The claims are false. The claims are not implied by the premises and Automated
//!   Reasoning policy. Furthermore, there exists different claims that are consistent with
//!   the premises and Automated Reasoning policy. SATISFIABLE - The claims can be true or
//!   false. It depends on what …
//! * `guardContent` (AutomatedReasoningPolicyTestGuardContent, len ..2048) **required** — The
//!   output content that's validated by the Automated Reasoning policy. This represents the
//!   foundation model response that will be checked for accuracy.
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy for
//!   which to create the test.
//! * `queryContent` (AutomatedReasoningPolicyTestQueryContent, len ..1024) — The input query
//!   or prompt that generated the content. This provides context for the validation.
//!
//! ## Output
//!
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the policy for which the test was
//!   created.
//! * `testCaseId` (AutomatedReasoningPolicyTestCaseId, len ..12, pattern ^[0-9A-Z]{12}$)
//!   **required** — The unique identifier of the created test.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `ConflictException` (HTTP 400) — Error occurred because of a conflict while performing
//!   an operation.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ServiceQuotaExceededException` (HTTP 400) — The number of requests exceeds the service
//!   quota. Resubmit your request later.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateAutomatedReasoningPolicyTestCaseError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateAutomatedReasoningPolicyTestCaseError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#CreateAutomatedReasoningPolicyTestCase")]
#[derive(Debug)]
pub enum CreateAutomatedReasoningPolicyTestCaseErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_automated_reasoning_policy_test_case::CreateAutomatedReasoningPolicyTestCaseError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateAutomatedReasoningPolicyTestCaseError = crate::scaffolded::Scaffolded<CreateAutomatedReasoningPolicyTestCaseErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateAutomatedReasoningPolicyTestCaseErrorModeled> for CreateAutomatedReasoningPolicyTestCaseError {
    fn from(modeled: CreateAutomatedReasoningPolicyTestCaseErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateAutomatedReasoningPolicyTestCase` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateAutomatedReasoningPolicyTestCase<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateAutomatedReasoningPolicyTestCase<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateAutomatedReasoningPolicyTestCase<Eng>
{
    type Input = CreateAutomatedReasoningPolicyTestCaseInput;
    type Output = CreateAutomatedReasoningPolicyTestCaseOutput;
    type Error = CreateAutomatedReasoningPolicyTestCaseError;

    const NAME: &'static str = "CreateAutomatedReasoningPolicyTestCase";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_AUTOMATED_REASONING_POLICY_TEST_CASE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_AUTOMATED_REASONING_POLICY_TEST_CASE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

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

        // The ARN is matched as the caller spelled it, exactly as a read of the policy
        // matches it, so an ARN of another region — and a versioned ARN of a version
        // nothing has minted — names no policy this account holds.
        if store.automated_reasoning_policy(&input.policy_arn).is_none() {
            return Err(not_found(&input.policy_arn));
        }

        // `@idempotencyToken`: a replay answers with what the first request minted.
        if let Some(minted) = input
            .client_request_token
            .as_ref()
            .and_then(|token| {
                store
                    .automated_reasoning_policy_test_case_create_tokens
                    .get(&(input.policy_arn.clone(), token.clone()))
            })
            .cloned()
        {
            if store
                .automated_reasoning_policy_test_case(&input.policy_arn, &minted)
                .is_some()
            {
                return Ok(CreateAutomatedReasoningPolicyTestCaseOutput {
                    policy_arn: input.policy_arn,
                    test_case_id: minted,
                });
            }
        }

        // The policy's test quota. Enforcing it here is also what bounds this part of the
        // store: a test carries no name a second create could collide with, so without a
        // server-side cap a caller could grow it with identical requests.
        let held = store.automated_reasoning_policy_test_case_count(&input.policy_arn);
        if held >= crate::state::MAX_AUTOMATED_REASONING_POLICY_TEST_CASES_PER_POLICY {
            return Err(
                CreateAutomatedReasoningPolicyTestCaseErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: Some(format!(
                            "this policy already holds the maximum of {} tests; delete one \
                             or request a quota increase",
                            crate::state::MAX_AUTOMATED_REASONING_POLICY_TEST_CASES_PER_POLICY
                        )),
                    },
                )
                .into(),
            );
        }

        let id = store.mint_automated_reasoning_policy_test_case_id(now);
        let record = crate::state::AutomatedReasoningPolicyTestCaseRecord {
            id: id.clone(),
            guard_content: input.guard_content,
            query_content: input.query_content,
            expected_aggregated_findings_result: input.expected_aggregated_findings_result,
            confidence_threshold: input.confidence_threshold,
            created_at: now,
            updated_at: now,
        };
        if let Some(token) = input.client_request_token {
            store
                .automated_reasoning_policy_test_case_create_tokens
                .insert((input.policy_arn.clone(), token), id.clone());
        }
        store
            .automated_reasoning_policy_test_cases
            .insert((input.policy_arn.clone(), id.clone()), record);

        // Read the identity back out of the STORED test, so what the caller is told is
        // exactly what a later GetAutomatedReasoningPolicyTestCase will find.
        let Some(stored) = store.automated_reasoning_policy_test_case(&input.policy_arn, &id) else {
            return Err(server_error());
        };
        Ok(CreateAutomatedReasoningPolicyTestCaseOutput {
            test_case_id: stored.id.clone(),
            policy_arn: input.policy_arn,
        })
    }
}

/// The modelled 404 this operation answers for a `policyArn` this account holds no policy
/// under — the test is created UNDER a policy, so there is nothing to create one on.
fn not_found(policy_arn: &str) -> CreateAutomatedReasoningPolicyTestCaseError {
    CreateAutomatedReasoningPolicyTestCaseErrorModeled::ResourceNotFoundException(
        ResourceNotFoundException {
            message: Some(format!(
                "no automated reasoning policy {policy_arn} exists in this account"
            )),
        },
    )
    .into()
}

/// The modelled 500 this operation answers when the engine carries no store for this
/// service — never a fresh engine, which would drop the test on return.
fn server_error() -> CreateAutomatedReasoningPolicyTestCaseError {
    CreateAutomatedReasoningPolicyTestCaseErrorModeled::InternalServerException(
        InternalServerException {
            message: Some(
                "automated reasoning policy state is not available on this engine".to_string(),
            ),
        },
    )
    .into()
}
