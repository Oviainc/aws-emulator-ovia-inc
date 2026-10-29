// SPDX-License-Identifier: CC0-1.0

//! @op StartAutomatedReasoningPolicyTestWorkflow
//!
//! Schedules the stored tests of an Automated Reasoning policy against one of its build
//! workflows and PERSISTS one run per test in [`crate::state`]: the policy, the build
//! workflow and the test each run ran, the status it holds, and when it last changed. The
//! runs this schedules are exactly the results `GetAutomatedReasoningPolicyTestResult`
//! reads back.
//!
//! Which tests run is what the request names: the `testCaseIds` it carried, or every test
//! stored under the policy when it carried none. A run starts `SCHEDULED` and the clock
//! owes it `COMPLETED` ([`crate::state::TEST_RUN_COMPLETE_AFTER`]), so a caller reading
//! straight after the start still sees the status the start accepted.
//!
//! A policy or a build workflow this account holds none of is refused with
//! `ResourceNotFoundException`; a build workflow whose tests are still running is refused
//! with `ResourceInUseException`; a build workflow that does not show `COMPLETED`, a
//! `testCaseIds` naming a test the policy does not hold, and a policy with no test to run
//! at all are refused with `ValidationException`; and the request the same
//! `clientRequestToken` and the same selection already started schedules nothing new
//! (`@idempotencyToken`).
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `StartAutomatedReasoningPolicyTestWorkflow`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Initiates a test workflow to validate Automated Reasoning policy tests. The workflow
//! executes the specified tests against the policy and generates validation results.
//!
//! ## Input
//!
//! * `buildWorkflowId` (AutomatedReasoningPolicyBuildWorkflowId, len ..36, pattern
//!   ^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$) **required**
//!   — The build workflow identifier. The build workflow must show a COMPLETED status before
//!   running tests.
//! * `clientRequestToken` (IdempotencyToken, len 1..256, pattern
//!   ^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$, @idempotencyToken) — A unique,
//!   case-sensitive identifier to ensure that the operation completes no more than one time.
//!   If this token matches a previous request, Amazon Bedrock ignores the request but doesn't
//!   return an error.
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy to test.
//! * `testCaseIds` (AutomatedReasoningPolicyTestCaseIdList, len 1..1) — The list of test
//!   identifiers to run. If not provided, all tests for the policy are run.
//!
//! ## Output
//!
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the policy for which the test workflow
//!   was started.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
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

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `StartAutomatedReasoningPolicyTestWorkflowError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StartAutomatedReasoningPolicyTestWorkflowError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#StartAutomatedReasoningPolicyTestWorkflow")]
#[derive(Debug)]
pub enum StartAutomatedReasoningPolicyTestWorkflowErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_automated_reasoning_policy_test_workflow::StartAutomatedReasoningPolicyTestWorkflowError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StartAutomatedReasoningPolicyTestWorkflowError = crate::scaffolded::Scaffolded<StartAutomatedReasoningPolicyTestWorkflowErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StartAutomatedReasoningPolicyTestWorkflowErrorModeled> for StartAutomatedReasoningPolicyTestWorkflowError {
    fn from(modeled: StartAutomatedReasoningPolicyTestWorkflowErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartAutomatedReasoningPolicyTestWorkflow` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StartAutomatedReasoningPolicyTestWorkflow<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartAutomatedReasoningPolicyTestWorkflow<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartAutomatedReasoningPolicyTestWorkflow<Eng>
{
    type Input = StartAutomatedReasoningPolicyTestWorkflowInput;
    type Output = StartAutomatedReasoningPolicyTestWorkflowOutput;
    type Error = StartAutomatedReasoningPolicyTestWorkflowError;

    const NAME: &'static str = "StartAutomatedReasoningPolicyTestWorkflow";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_AUTOMATED_REASONING_POLICY_TEST_WORKFLOW_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_AUTOMATED_REASONING_POLICY_TEST_WORKFLOW_OUTPUT_SCHEMA;
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
        // nothing has minted — names no policy whose tests could run.
        if store.automated_reasoning_policy(&input.policy_arn).is_none() {
            return Err(
                StartAutomatedReasoningPolicyTestWorkflowErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(format!(
                            "no automated reasoning policy {} exists in this account",
                            input.policy_arn
                        )),
                    },
                )
                .into(),
            );
        }

        // The build workflow is read as of `now`, so one the clock has already completed
        // is terminal for the checks below.
        let Some(workflow) = store.automated_reasoning_policy_build_workflow(
            &input.policy_arn,
            &input.build_workflow_id,
            now,
        ) else {
            return Err(
                StartAutomatedReasoningPolicyTestWorkflowErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(crate::state::build_workflow_not_found_message().to_string()),
                    },
                )
                .into(),
            );
        };
        let workflow_status = workflow.status;

        // A build workflow that has not built the policy has nothing to test — whether it
        // is still working or it wound down without completing — which is a request the
        // caller must change rather than retry: `buildWorkflowId`'s own documentation is
        // that the build workflow must show a COMPLETED status before running tests
        // (`smithy_query(query="doc",shape="StartAutomatedReasoningPolicyTestWorkflowRequest",member="buildWorkflowId")`).
        //
        // The wording is the service's own for that refusal, as the corpus
        // `simparity-behavioral-ga-prod-v1` records Amazon Bedrock answering it, whose only
        // variable part is the id the request carried.
        if workflow_status != crate::state::BUILD_WORKFLOW_STATUS_COMPLETED {
            return Err(validation(format!(
                "Build workflow: {} is not in {} status",
                input.build_workflow_id,
                crate::state::BUILD_WORKFLOW_STATUS_COMPLETED
            )));
        }

        // Which tests this workflow runs: the ones the request named, or every test stored
        // under the policy when it named none. The two refusals below are the service's
        // own wording for a selection it cannot run; neither is recorded by the corpus, so
        // each states what is wrong with the request and names the value it came from.
        let selection = input.test_case_ids.clone();
        let test_case_ids: Vec<String> = match input.test_case_ids {
            Some(named) => {
                for test_case_id in &named {
                    if store
                        .automated_reasoning_policy_test_case(&input.policy_arn, test_case_id)
                        .is_none()
                    {
                        return Err(validation(format!(
                            "Test case: {test_case_id} not found"
                        )));
                    }
                }
                named
            }
            None => store
                .automated_reasoning_policy_test_cases_of(&input.policy_arn)
                .into_iter()
                .map(|test_case| test_case.id.clone())
                .collect(),
        };
        if test_case_ids.is_empty() {
            return Err(validation(format!(
                "Policy: {} has no test cases to run",
                input.policy_arn
            )));
        }

        // `@idempotencyToken`: this IS the test workflow this build workflow last accepted
        // when the token and the selection are the ones it accepted, so it schedules nothing
        // new and is not an error. The selection is part of that comparison because a
        // request naming other tests is a request for other work, which a stored token must
        // not answer as already done. Every check above still applies to it — only the
        // in-use refusal below does not, because the runs it would refuse on are the ones
        // this very request started.
        if let Some(token) = input.client_request_token.as_ref() {
            if store
                .automated_reasoning_policy_test_workflow_tokens
                .get(&(input.policy_arn.clone(), input.build_workflow_id.clone()))
                .is_some_and(|accepted| {
                    accepted.client_request_token == *token
                        && accepted.test_case_ids == selection
                })
            {
                return Ok(StartAutomatedReasoningPolicyTestWorkflowOutput {
                    policy_arn: input.policy_arn,
                });
            }
        }

        // Every run of this build workflow is read as of `now` too, so a run the clock has
        // already completed no longer holds the build workflow.
        store.settle_automated_reasoning_policy_test_runs(
            &input.policy_arn,
            &input.build_workflow_id,
            now,
        );
        if store
            .automated_reasoning_policy_test_runs_of(&input.policy_arn, &input.build_workflow_id)
            .into_iter()
            .any(|run| run.is_active())
        {
            return Err(
                StartAutomatedReasoningPolicyTestWorkflowErrorModeled::ResourceInUseException(
                    ResourceInUseException {
                        message: Some(
                            crate::state::AUTOMATED_REASONING_POLICY_TEST_WORKFLOW_IN_PROGRESS
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        }

        // One run per test, scheduled as of `now`. A test this build workflow has already
        // run is RE-run: its run is the one result this pair of resources has, so the new
        // run replaces the outcome of the old one rather than adding a second record.
        for test_case_id in &test_case_ids {
            let key = (
                input.policy_arn.clone(),
                input.build_workflow_id.clone(),
                test_case_id.clone(),
            );
            store
                .automated_reasoning_policy_test_runs
                .entry(key)
                .or_insert_with(|| {
                    crate::state::AutomatedReasoningPolicyTestRunRecord::scheduled(
                        &input.policy_arn,
                        &input.build_workflow_id,
                        test_case_id,
                        now,
                    )
                })
                .schedule(now);
        }

        if let Some(token) = input.client_request_token {
            store
                .automated_reasoning_policy_test_workflow_tokens
                .insert(
                    (
                        input.policy_arn.clone(),
                        input.build_workflow_id.clone(),
                    ),
                    crate::state::AcceptedAutomatedReasoningPolicyTestWorkflow {
                        client_request_token: token,
                        test_case_ids: selection,
                    },
                );
        }

        Ok(StartAutomatedReasoningPolicyTestWorkflowOutput {
            policy_arn: input.policy_arn,
        })
    }
}

/// The modelled 400 `ValidationException` this operation refuses a request with that every
/// modelled constraint admits but this service does not: a build workflow that never built
/// the policy, a named test the policy does not hold, or a policy with no test to run.
fn validation(message: String) -> StartAutomatedReasoningPolicyTestWorkflowError {
    StartAutomatedReasoningPolicyTestWorkflowErrorModeled::ValidationException(
        ValidationException {
            message: Some(message),
        },
    )
    .into()
}

/// The modelled 500 this operation answers when the engine carries no store for this
/// service — never a fresh engine, which would drop the runs on return.
fn server_error() -> StartAutomatedReasoningPolicyTestWorkflowError {
    StartAutomatedReasoningPolicyTestWorkflowErrorModeled::InternalServerException(
        InternalServerException {
            message: Some(
                "automated reasoning policy state is not available on this engine".to_string(),
            ),
        },
    )
    .into()
}

