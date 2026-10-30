// SPDX-License-Identifier: CC0-1.0

//! @op UpdateAutomatedReasoningPolicyTestCase
//!
//! Rewrites one stored test of an Automated Reasoning policy in [`crate::state`], addressed
//! by the `(policyArn, testCaseId)` pair every operation on a test is addressed by.
//!
//! The pair is resolved against the store FIRST, through the same lookup
//! `GetAutomatedReasoningPolicyTestCase` reads a test with and with that resource's own
//! wording ([`crate::state::automated_reasoning_policy_test_case_not_found_message`]): a
//! request whose members satisfy every modelled constraint but names a test this account
//! holds none of has failed on the IDENTITY it addressed, so it is
//! `ResourceNotFoundException` (404) — the same answer a read of that pair gives, whether
//! the test never existed or a delete has already removed it.
//!
//! `lastUpdatedAt` is the concurrency token the member documents, so it is weighed only
//! once the record exists: a token naming an instant other than the write the test
//! currently holds is `ConflictException` (400), exactly as the delete of a test weighs it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateAutomatedReasoningPolicyTestCase`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Updates an existing Automated Reasoning policy test. You can modify the content, query,
//! expected result, and confidence threshold.
//!
//! ## Input
//!
//! * `clientRequestToken` (IdempotencyToken, len 1..256, pattern
//!   ^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$, @idempotencyToken) — A unique,
//!   case-sensitive identifier to ensure that the operation completes no more than one time.
//!   If this token matches a previous request, Amazon Bedrock ignores the request, but does
//!   not return an error.
//! * `confidenceThreshold` (AutomatedReasoningCheckTranslationConfidence, range 0..1) — The
//!   updated minimum confidence level for logic validation. If null is provided, the
//!   threshold will be removed.
//! * `expectedAggregatedFindingsResult` (AutomatedReasoningCheckResult, one of: IMPOSSIBLE |
//!   INVALID | NO_TRANSLATION | SATISFIABLE | TOO_COMPLEX | TRANSLATION_AMBIGUOUS | VALID)
//!   **required** — The updated expected result of the Automated Reasoning check.
//! * `guardContent` (AutomatedReasoningPolicyTestGuardContent, len ..2048) **required** — The
//!   updated content to be validated by the Automated Reasoning policy.
//! * `lastUpdatedAt` (Timestamp) **required** — The timestamp when the test was last updated.
//!   This is used as a concurrency token to prevent conflicting modifications.
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy that
//!   contains the test.
//! * `queryContent` (AutomatedReasoningPolicyTestQueryContent, len ..1024) — The updated
//!   input query or prompt that generated the content.
//! * `testCaseId` (AutomatedReasoningPolicyTestCaseId, len ..12, pattern ^[0-9A-Z]{12}$)
//!   **required** — The unique identifier of the test to update.
//!
//! ## Output
//!
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the policy that contains the updated
//!   test.
//! * `testCaseId` (AutomatedReasoningPolicyTestCaseId, len ..12, pattern ^[0-9A-Z]{12}$)
//!   **required** — The unique identifier of the updated test.
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
/// ([`crate::state::automated_reasoning_policy_test_case_conflict_message`]).
const UPDATING: &str = "updating";

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateAutomatedReasoningPolicyTestCaseError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateAutomatedReasoningPolicyTestCaseError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#UpdateAutomatedReasoningPolicyTestCase")]
#[derive(Debug)]
pub enum UpdateAutomatedReasoningPolicyTestCaseErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_automated_reasoning_policy_test_case::UpdateAutomatedReasoningPolicyTestCaseError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateAutomatedReasoningPolicyTestCaseError = crate::scaffolded::Scaffolded<UpdateAutomatedReasoningPolicyTestCaseErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateAutomatedReasoningPolicyTestCaseErrorModeled> for UpdateAutomatedReasoningPolicyTestCaseError {
    fn from(modeled: UpdateAutomatedReasoningPolicyTestCaseErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateAutomatedReasoningPolicyTestCase` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateAutomatedReasoningPolicyTestCase<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateAutomatedReasoningPolicyTestCase<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateAutomatedReasoningPolicyTestCase<Eng>
{
    type Input = UpdateAutomatedReasoningPolicyTestCaseInput;
    type Output = UpdateAutomatedReasoningPolicyTestCaseOutput;
    type Error = UpdateAutomatedReasoningPolicyTestCaseError;

    const NAME: &'static str = "UpdateAutomatedReasoningPolicyTestCase";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_AUTOMATED_REASONING_POLICY_TEST_CASE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_AUTOMATED_REASONING_POLICY_TEST_CASE_OUTPUT_SCHEMA;
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

        // IDENTITY FIRST. The ARN is matched as the caller spelled it, exactly as a read of
        // the policy matches it, so an ARN of another region — and a versioned ARN of a
        // version nothing has minted — names no policy this account holds, and a test hangs
        // off the policy it was created under.
        if store.automated_reasoning_policy(&input.policy_arn).is_none() {
            return Err(not_found(
                crate::state::AUTOMATED_REASONING_POLICY_NOT_FOUND.to_string(),
            ));
        }
        let Some(stored) =
            store.automated_reasoning_policy_test_case(&input.policy_arn, &input.test_case_id)
        else {
            return Err(not_found(
                crate::state::automated_reasoning_policy_test_case_not_found_message(
                    &input.test_case_id,
                    &input.policy_arn,
                ),
            ));
        };
        let id = stored.id.clone();
        let held = stored.updated_at;
        let applied_token = stored.update_client_request_token.clone();

        // `clientRequestToken` "ensure[s] that the operation completes no more than one
        // time. If this token matches a previous request, Amazon Bedrock ignores the
        // request, but does not return an error": a retry carries the token of the write
        // that already landed TOGETHER with the `lastUpdatedAt` the retried request was
        // built from, which is no longer the one the test holds — so the token is weighed
        // ahead of the concurrency token, and the replay is answered rather than refused.
        if input.client_request_token.is_some() && input.client_request_token == applied_token {
            return Ok(UpdateAutomatedReasoningPolicyTestCaseOutput {
                policy_arn: input.policy_arn,
                test_case_id: id,
            });
        }

        // `lastUpdatedAt` is the concurrency token the member documents, and it is about a
        // record that EXISTS: the update goes through only while it names the write the
        // test currently holds, so an update issued against a test somebody has written
        // since is refused instead of overwriting that write.
        if !crate::state::names_write_at(&input.last_updated_at, held) {
            return Err(conflict(
                crate::state::automated_reasoning_policy_test_case_conflict_message(
                    UPDATING,
                    &input.test_case_id,
                    &input.policy_arn,
                ),
            ));
        }

        let Some(record) = store
            .automated_reasoning_policy_test_cases
            .get_mut(&(input.policy_arn.clone(), input.test_case_id.clone()))
        else {
            return Err(server_error());
        };
        // The request carries the test's content members in full — both required ones and
        // both optional ones — so each is replaced by what it carried, and an optional
        // member the request omits is REMOVED from the test rather than left behind:
        // `confidenceThreshold` documents exactly that ("If null is provided, the threshold
        // will be removed"), and `queryContent` is the same kind of member of the same
        // request. `createdAt` and the id are the test's identity and are not rewritten.
        record.guard_content = input.guard_content;
        record.query_content = input.query_content;
        record.expected_aggregated_findings_result = input.expected_aggregated_findings_result;
        record.confidence_threshold = input.confidence_threshold;
        record.update_client_request_token = input.client_request_token;
        record.updated_at = now;

        Ok(UpdateAutomatedReasoningPolicyTestCaseOutput {
            test_case_id: record.id.clone(),
            policy_arn: input.policy_arn,
        })
    }
}

/// The modelled 404 this operation answers for a pair this account holds no test under —
/// the test is addressed by the PAIR, so either half missing is "the resource was not
/// found", and a mutation of a missing resource fails on its identity rather than on its
/// parameters.
fn not_found(message: String) -> UpdateAutomatedReasoningPolicyTestCaseError {
    UpdateAutomatedReasoningPolicyTestCaseErrorModeled::ResourceNotFoundException(
        ResourceNotFoundException {
            message: Some(message),
        },
    )
    .into()
}

/// The modelled 400 this operation answers when `lastUpdatedAt` does not name the write the
/// addressed test holds.
fn conflict(message: String) -> UpdateAutomatedReasoningPolicyTestCaseError {
    UpdateAutomatedReasoningPolicyTestCaseErrorModeled::ConflictException(ConflictException {
        message: Some(message),
    })
    .into()
}

/// The modelled 500 this operation answers when the engine carries no store for this
/// service — never a fresh engine, which would drop the write on return.
fn server_error() -> UpdateAutomatedReasoningPolicyTestCaseError {
    UpdateAutomatedReasoningPolicyTestCaseErrorModeled::InternalServerException(
        InternalServerException {
            message: Some(
                "automated reasoning policy state is not available on this engine".to_string(),
            ),
        },
    )
    .into()
}
