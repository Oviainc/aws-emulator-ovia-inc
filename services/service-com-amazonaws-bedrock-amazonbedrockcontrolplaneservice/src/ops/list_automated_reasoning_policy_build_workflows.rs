// SPDX-License-Identifier: CC0-1.0

//! @op ListAutomatedReasoningPolicyBuildWorkflows
//!
//! Enumerates the build workflows stored under one Automated Reasoning policy — the
//! workflows `ops/start_automated_reasoning_policy_build_workflow.rs` started, each as the
//! summary of what `ops/get_automated_reasoning_policy_build_workflow.rs` reports for that
//! one workflow, so a listed workflow and a read workflow agree member for member.
//!
//! The rules it follows:
//!
//! * SCOPE. The policy ARN is resolved exactly as a read of the policy itself resolves it,
//!   so an ARN this account holds no policy for is 404 `ResourceNotFoundException` rather
//!   than an empty listing: a listing of nothing and a listing of a policy that does not
//!   exist are different answers. A policy nothing has built yet reports the EMPTY list,
//!   which the required `@length(min: 0)` member admits.
//! * ORDER. The operation models neither a `sortBy` nor a `sortOrder`, so it has ONE
//!   order, and its doc calls the listing "the history of policy creation and modification
//!   attempts": a history in the order it happened, oldest first, with the workflow id
//!   breaking a tie so the order is total.
//! * PAGES. `maxResults` bounds one page and `nextToken` resumes the next, as the cursor
//!   of the first row the page did not carry — the [`crate::state::listing_cursor`] every
//!   creation-time listing of this service pages with — so the pages partition the
//!   workflows with neither a gap nor a repeat even when one is started in between. A
//!   token that cursor does not render ([`crate::state::cursor_belongs`]) is refused 400
//!   `ValidationException`.
//!
//! A REPORTING operation (`@readonly`): the one write it makes is
//! [`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::settle_automated_reasoning_policy_build_workflows`],
//! which materialises the status transitions the clock had already made due at the same
//! instant even if nobody had called — so the listing reports the status a read of each
//! workflow would report.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListAutomatedReasoningPolicyBuildWorkflows`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Lists all build workflows for an Automated Reasoning policy, showing the history of
//! policy creation and modification attempts.
//!
//! ## Input
//!
//! * `maxResults` (MaxResults, range 1..1000, default 25) **required** — The maximum number
//!   of build workflows to return in a single response. Valid range is 1-100.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — A pagination token from a
//!   previous request to continue listing build workflows from where the previous request
//!   left off.
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy whose
//!   build workflows you want to list.
//!
//! ## Output
//!
//! * `automatedReasoningPolicyBuildWorkflowSummaries`
//!   (AutomatedReasoningPolicyBuildWorkflowSummaries, len 0..1000) **required** — A list of
//!   build workflow summaries, each containing key information about a build workflow
//!   including its status and timestamps.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — A pagination token to use in
//!   subsequent requests to retrieve additional build workflows.
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
// `ListAutomatedReasoningPolicyBuildWorkflowsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListAutomatedReasoningPolicyBuildWorkflowsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#ListAutomatedReasoningPolicyBuildWorkflows")]
#[derive(Debug)]
pub enum ListAutomatedReasoningPolicyBuildWorkflowsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_automated_reasoning_policy_build_workflows::ListAutomatedReasoningPolicyBuildWorkflowsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListAutomatedReasoningPolicyBuildWorkflowsError = crate::scaffolded::Scaffolded<ListAutomatedReasoningPolicyBuildWorkflowsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListAutomatedReasoningPolicyBuildWorkflowsErrorModeled> for ListAutomatedReasoningPolicyBuildWorkflowsError {
    fn from(modeled: ListAutomatedReasoningPolicyBuildWorkflowsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListAutomatedReasoningPolicyBuildWorkflows` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListAutomatedReasoningPolicyBuildWorkflows<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListAutomatedReasoningPolicyBuildWorkflows<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The largest page this listing answers with:
/// `AutomatedReasoningPolicyBuildWorkflowSummaries` is modelled `len 0..1000` and
/// `maxResults` `range 1..1000`, so 1000 is the whole span either admits.
const MAX_PAGE_SIZE: usize = 1000;

/// The page size a request that names none takes: the `maxResults` member's own modelled
/// `default`
/// (`smithy_query(query="operation",operation="ListAutomatedReasoningPolicyBuildWorkflows")`).
const DEFAULT_PAGE_SIZE: usize = 25;

/// How a stored build workflow is summarised: member for member what a read of that one
/// workflow reports, minus the source document members the summary does not model.
fn summary(
    record: &crate::state::AutomatedReasoningPolicyBuildWorkflowRecord,
) -> AutomatedReasoningPolicyBuildWorkflowSummary {
    AutomatedReasoningPolicyBuildWorkflowSummary {
        policy_arn: record.policy_arn.clone(),
        build_workflow_id: record.id.clone(),
        status: record.status.to_string(),
        build_workflow_type: record.workflow_type.clone(),
        created_at: crate::state::date_time(record.created_at),
        updated_at: crate::state::date_time(record.updated_at),
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListAutomatedReasoningPolicyBuildWorkflows<Eng>
{
    type Input = ListAutomatedReasoningPolicyBuildWorkflowsInput;
    type Output = ListAutomatedReasoningPolicyBuildWorkflowsOutput;
    type Error = ListAutomatedReasoningPolicyBuildWorkflowsError;

    const NAME: &'static str = "ListAutomatedReasoningPolicyBuildWorkflows";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_AUTOMATED_REASONING_POLICY_BUILD_WORKFLOWS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_AUTOMATED_REASONING_POLICY_BUILD_WORKFLOWS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                ListAutomatedReasoningPolicyBuildWorkflowsErrorModeled::InternalServerException(
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

        // The policy is resolved as the caller spelled it, exactly as a read of the policy
        // itself resolves it: a policy this account holds none of is not an empty listing.
        if store.automated_reasoning_policy(&input.policy_arn).is_none() {
            return Err(
                ListAutomatedReasoningPolicyBuildWorkflowsErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(
                            crate::state::AUTOMATED_REASONING_POLICY_NOT_FOUND.to_string(),
                        ),
                    },
                )
                .into(),
            );
        }

        // The status each workflow holds NOW is what this listing reports, so the
        // transitions the clock has already made due are materialised before it reads
        // them — the very ones a read of each workflow would have materialised.
        store.settle_automated_reasoning_policy_build_workflows(&input.policy_arn, now);

        let mut rows: Vec<(String, AutomatedReasoningPolicyBuildWorkflowSummary)> = store
            .automated_reasoning_policy_build_workflows_of(&input.policy_arn)
            .map(|record| {
                (
                    crate::state::listing_cursor(record.created_at, &record.id),
                    summary(record),
                )
            })
            .collect();
        // The cursor's leading half IS the instant the workflow was started and its
        // trailing half is the workflow id, so byte order over cursors is the
        // `(createdAt, buildWorkflowId)` order this listing walks — a TOTAL order, which
        // is what lets its pages partition the workflows.
        rows.sort_by(|(left, _), (right, _)| left.cmp(right));

        if let Some(token) = input.next_token.as_deref() {
            if !crate::state::cursor_belongs(token) {
                return Err(
                    ListAutomatedReasoningPolicyBuildWorkflowsErrorModeled::ValidationException(
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
            // A well-formed cursor names a POSITION in that order, so a page resumes at
            // the first workflow at or after it whatever happened in between.
            rows.retain(|(cursor, _)| cursor.as_str() >= token);
        }

        // An absent `maxResults` arrives as the member's own modelled default, and
        // anything the constraint walk admitted is already inside the modelled range; the
        // clamp is the shape every list operation of this crate computes a page size with.
        let page_size = usize::try_from(input.max_results)
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        // The workflow the next page starts on IS the next page's cursor, so the pages
        // partition the listing with neither a gap nor a repeat.
        let next_token = rows.get(page_size).map(|(cursor, _)| cursor.clone());
        rows.truncate(page_size);

        Ok(ListAutomatedReasoningPolicyBuildWorkflowsOutput {
            automated_reasoning_policy_build_workflow_summaries: rows
                .into_iter()
                .map(|(_, summary)| summary)
                .collect(),
            next_token,
        })
    }
}
