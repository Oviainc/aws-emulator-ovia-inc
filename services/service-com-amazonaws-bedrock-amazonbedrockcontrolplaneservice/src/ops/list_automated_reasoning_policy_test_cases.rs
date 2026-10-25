// SPDX-License-Identifier: CC0-1.0

//! @op ListAutomatedReasoningPolicyTestCases
//!
//! Enumerates the tests stored under one Automated Reasoning policy as the
//! `AutomatedReasoningPolicyTestCase` items a read of each test reports — the ONE mapping
//! in [`crate::state::automated_reasoning_policy_test_case`], so a listed test and a read
//! test agree member for member.
//!
//! A REPORTING operation: it writes nothing a later caller sees.
//!
//! The policy ARN is resolved exactly as a read of the policy itself resolves it, so a
//! policy this account holds none of is 404 `ResourceNotFoundException` rather than an
//! empty listing: a listing of nothing and a listing of a policy that does not exist are
//! different answers.
//!
//! Pagination is a cursor over the total order the listing walks — the `testCaseId`, which
//! is unique within a policy — so a page resumes at the first test at or after the cursor
//! whatever was created or deleted in between, and the pages partition the tests with
//! neither a gap nor a repeat. `@paginated` names `nextToken`/`testCases`.
//!
//! ---
//!
//! # Modeled contract for `ListAutomatedReasoningPolicyTestCases`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Lists tests for an Automated Reasoning policy. We recommend using pagination to ensure
//! that the operation returns quickly and successfully.
//!
//! ## Input
//!
//! * `maxResults` (MaxResults, range 1..1000, default 25) **required** — The maximum number
//!   of tests to return in a single call.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — The pagination token from a
//!   previous request to retrieve the next page of results.
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy for
//!   which to list tests.
//!
//! ## Output
//!
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — The pagination token to use
//!   in a subsequent request to retrieve the next page of results.
//! * `testCases` (AutomatedReasoningPolicyTestCaseList, len 0..1000) **required** — A list of
//!   tests for the specified policy.
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
// `ListAutomatedReasoningPolicyTestCasesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListAutomatedReasoningPolicyTestCasesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#ListAutomatedReasoningPolicyTestCases")]
#[derive(Debug)]
pub enum ListAutomatedReasoningPolicyTestCasesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_automated_reasoning_policy_test_cases::ListAutomatedReasoningPolicyTestCasesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListAutomatedReasoningPolicyTestCasesError = crate::scaffolded::Scaffolded<ListAutomatedReasoningPolicyTestCasesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListAutomatedReasoningPolicyTestCasesErrorModeled> for ListAutomatedReasoningPolicyTestCasesError {
    fn from(modeled: ListAutomatedReasoningPolicyTestCasesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListAutomatedReasoningPolicyTestCases` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListAutomatedReasoningPolicyTestCases<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListAutomatedReasoningPolicyTestCases<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The largest page this listing answers with: `AutomatedReasoningPolicyTestCaseList` is
/// modelled `len 0..1000` and `maxResults` `range 1..1000`, so 1000 is the whole span
/// either admits.
const MAX_PAGE_SIZE: usize = 1000;

/// The page size a request that names none takes: the `maxResults` member's own modelled
/// `default` (`smithy_query(query="operation",operation="ListAutomatedReasoningPolicyTestCases")`).
const DEFAULT_PAGE_SIZE: usize = 25;

/// Whether `token` has the FORM of a cursor this listing issues: a `testCaseId`
/// (`AutomatedReasoningPolicyTestCaseId` is `^[0-9A-Z]{12}$`), which is what its pages are
/// cut on.
///
/// The form is all the cursor's own grammar can decide, and all it needs to: a well-formed
/// token names a POSITION in the id order — the page resumes at the first test at or after
/// it — so one carried over from another policy's listing resumes at that position rather
/// than being refused, exactly as a token whose own test has since been deleted does. What
/// the check rules out is a token this listing could never have issued, which would
/// otherwise be read as an empty or arbitrary page.
fn cursor_shaped(token: &str) -> bool {
    token.len() == 12
        && token
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListAutomatedReasoningPolicyTestCases<Eng>
{
    type Input = ListAutomatedReasoningPolicyTestCasesInput;
    type Output = ListAutomatedReasoningPolicyTestCasesOutput;
    type Error = ListAutomatedReasoningPolicyTestCasesError;

    const NAME: &'static str = "ListAutomatedReasoningPolicyTestCases";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_AUTOMATED_REASONING_POLICY_TEST_CASES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_AUTOMATED_REASONING_POLICY_TEST_CASES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                ListAutomatedReasoningPolicyTestCasesErrorModeled::InternalServerException(
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

        // The policy is resolved as the caller spelled it, exactly as a read of the policy
        // itself resolves it: a policy this account holds none of is not an empty listing.
        if store.automated_reasoning_policy(&input.policy_arn).is_none() {
            return Err(
                ListAutomatedReasoningPolicyTestCasesErrorModeled::ResourceNotFoundException(
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

        let mut rows: Vec<AutomatedReasoningPolicyTestCase> = store
            .automated_reasoning_policy_test_cases_of(&input.policy_arn)
            .into_iter()
            .map(crate::state::automated_reasoning_policy_test_case)
            .collect();

        if let Some(token) = input.next_token.as_deref() {
            if !cursor_shaped(token) {
                return Err(
                    ListAutomatedReasoningPolicyTestCasesErrorModeled::ValidationException(
                        ValidationException {
                            message: Some(
                                "nextToken is not a pagination token this service issued"
                                    .to_string(),
                            ),
                        },
                    )
                    .into(),
                );
            }
            rows.retain(|test_case| test_case.test_case_id.as_str() >= token);
        }

        // An absent `maxResults` arrives as the member's own modelled default, and
        // anything the constraint walk admitted is already inside the modelled range; the
        // clamp is the shape every list operation of this crate computes a page size with.
        let page_size = usize::try_from(input.max_results)
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        // The test the next page starts on IS the next page's cursor, so the pages
        // partition the listing with neither a gap nor a repeat.
        let next_token = rows
            .get(page_size)
            .map(|test_case| test_case.test_case_id.clone());
        rows.truncate(page_size);

        Ok(ListAutomatedReasoningPolicyTestCasesOutput {
            test_cases: rows,
            next_token,
        })
    }
}
