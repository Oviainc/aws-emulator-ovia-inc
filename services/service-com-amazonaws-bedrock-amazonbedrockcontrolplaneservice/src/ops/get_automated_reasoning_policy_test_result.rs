// SPDX-License-Identifier: CC0-1.0

//! @op GetAutomatedReasoningPolicyTestResult
//!
//! Reports the run of ONE stored test against ONE build workflow of its policy, as
//! `StartAutomatedReasoningPolicyTestWorkflow` scheduled it in [`crate::state`]: the test
//! itself, the status the run holds and when it last changed. A read reports the status the
//! run holds at the instant it is asked, so the completion the clock already made due is
//! materialised first and two reads after that instant report ONE instant.
//!
//! A policy, a build workflow or a test this account holds none of is refused with
//! `ResourceNotFoundException`, as is a pair that no test workflow has run that test
//! against.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetAutomatedReasoningPolicyTestResult`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Retrieves the test result for a specific Automated Reasoning policy test. Returns
//! detailed validation findings and execution status.
//!
//! ## Input
//!
//! * `buildWorkflowId` (AutomatedReasoningPolicyBuildWorkflowId, len ..36, pattern
//!   ^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$) **required**
//!   — The build workflow identifier. The build workflow must display a COMPLETED status to
//!   get results.
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy.
//! * `testCaseId` (AutomatedReasoningPolicyTestCaseId, len ..12, pattern ^[0-9A-Z]{12}$)
//!   **required** — The unique identifier of the test for which to retrieve results.
//!
//! ## Output
//!
//! * `testResult` (AutomatedReasoningPolicyTestResult) **required** — The test result
//!   containing validation findings, execution status, and detailed analysis.
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
// `GetAutomatedReasoningPolicyTestResultError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetAutomatedReasoningPolicyTestResultError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#GetAutomatedReasoningPolicyTestResult")]
#[derive(Debug)]
pub enum GetAutomatedReasoningPolicyTestResultErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_automated_reasoning_policy_test_result::GetAutomatedReasoningPolicyTestResultError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetAutomatedReasoningPolicyTestResultError = crate::scaffolded::Scaffolded<GetAutomatedReasoningPolicyTestResultErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetAutomatedReasoningPolicyTestResultErrorModeled> for GetAutomatedReasoningPolicyTestResultError {
    fn from(modeled: GetAutomatedReasoningPolicyTestResultErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetAutomatedReasoningPolicyTestResult` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetAutomatedReasoningPolicyTestResult<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetAutomatedReasoningPolicyTestResult<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetAutomatedReasoningPolicyTestResult<Eng>
{
    type Input = GetAutomatedReasoningPolicyTestResultInput;
    type Output = GetAutomatedReasoningPolicyTestResultOutput;
    type Error = GetAutomatedReasoningPolicyTestResultError;

    const NAME: &'static str = "GetAutomatedReasoningPolicyTestResult";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_AUTOMATED_REASONING_POLICY_TEST_RESULT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_AUTOMATED_REASONING_POLICY_TEST_RESULT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                GetAutomatedReasoningPolicyTestResultErrorModeled::InternalServerException(
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

        // The policy ARN is matched as the caller spelled it, exactly as a read of the
        // policy itself matches it; the build workflow and the test are then looked up
        // under that policy, so neither an id of another policy's workflow nor an id of
        // another policy's test names anything here.
        if store.automated_reasoning_policy(&input.policy_arn).is_none() {
            return Err(not_found(format!(
                "no automated reasoning policy {} exists in this account",
                input.policy_arn
            )));
        }
        if store
            .automated_reasoning_policy_build_workflow(
                &input.policy_arn,
                &input.build_workflow_id,
                now,
            )
            .is_none()
        {
            return Err(not_found(
                crate::state::build_workflow_not_found_message().to_string(),
            ));
        }
        if store
            .automated_reasoning_policy_test_case(&input.policy_arn, &input.test_case_id)
            .is_none()
        {
            return Err(not_found(
                crate::state::automated_reasoning_policy_test_case_not_found_message(
                    &input.test_case_id,
                    &input.policy_arn,
                ),
            ));
        }

        // The run is read as of `now`, so the completion the workflow that scheduled it
        // owes it is already materialised — the one write this reporting operation makes,
        // and one that was owed at the due instant even if nobody had called. It is cloned
        // out of the store so the test it ran can be read beside it, and both come out of
        // the store, so a result reports exactly the test a read of that test reports and
        // exactly the run a later read of this result finds.
        let Some(run) = store
            .automated_reasoning_policy_test_run(
                &input.policy_arn,
                &input.build_workflow_id,
                &input.test_case_id,
                now,
            )
            .cloned()
        else {
            return Err(not_found(format!(
                "no test workflow has run test {} against build workflow {} of automated \
                 reasoning policy {}",
                input.test_case_id, input.build_workflow_id, input.policy_arn
            )));
        };
        let Some(test_case) =
            store.automated_reasoning_policy_test_case(&input.policy_arn, &input.test_case_id)
        else {
            return Err(not_found(
                crate::state::automated_reasoning_policy_test_case_not_found_message(
                    &input.test_case_id,
                    &input.policy_arn,
                ),
            ));
        };

        Ok(GetAutomatedReasoningPolicyTestResultOutput {
            test_result: crate::state::automated_reasoning_policy_test_result(&run, test_case),
        })
    }
}

/// The modelled 404 this operation answers for a policy, a build workflow, a test or a run
/// of that test this store holds none of — a result is addressed by the TRIPLE, so every
/// one of the four is "the resource was not found".
///
/// The wording for a missing build workflow is the service's own for that resource
/// ([`crate::state::build_workflow_not_found_message`]), and the wording for a missing test
/// is the service's own for THAT resource
/// ([`crate::state::automated_reasoning_policy_test_case_not_found_message`]), so a request
/// addressing either gets the same message wherever it came from.
fn not_found(message: String) -> GetAutomatedReasoningPolicyTestResultError {
    GetAutomatedReasoningPolicyTestResultErrorModeled::ResourceNotFoundException(
        ResourceNotFoundException {
            message: Some(message),
        },
    )
    .into()
}

