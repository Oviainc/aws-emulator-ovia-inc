// SPDX-License-Identifier: CC0-1.0

//! @op ListAutomationRulePreview
//! Returns a preview of the recommended actions that match an Automation rule's
//! configuration and criteria: the actions this service holds for the accounts the
//! rule type and `organizationScope` cover, restricted to the requested
//! `recommendedActionTypes` and to the `criteria` conditions, one page at a time.
//!
//! `@readonly`, so this handler writes NOTHING: the inventory it reports over is the
//! analysis output Compute Optimizer derives from observed resources
//! (`crate::state::RecommendedActionStore`), and a page token carries its own cursor.
//!
//! The refusals below are the EMULATOR's choice rather than modelled constraints, each
//! reported with the only client-input error this operation declares
//! (`InvalidParameterValueException`):
//!
//! * an `organizationScope` sent with a rule type other than `OrganizationRule`, because
//!   the scope names the accounts of an organization-wide preview and silently ignoring a
//!   member the caller sent would be worse than reporting it;
//! * an EMPTY `recommendedActionTypes`, because the member is required and names the
//!   action types to include, so an empty list selects nothing: answering an empty page
//!   would read as "your rule matches no resource" rather than "you asked for no action
//!   type";
//! * a `criteria` this service cannot evaluate — a condition with no `comparison`, with no
//!   `values`, with an operator from the wrong family, or a `resourceTag` condition with no
//!   `key` (the model types all of those as optional, so the protocol boundary admits
//!   them). None of them can select resources, so each is reported rather than quietly
//!   matching nothing.
//! * a `nextToken` that does not decode, or that was taken with a different rule type,
//!   scope, action-type list or criteria (or from another operation): resuming a walk
//!   with it would answer a page of a different result set, so the token is refused
//!   instead.
//!
//! NOT enforced here, deliberately: the model's prose for `ruleType` says only the
//! management account or a delegated administrator may ask for `OrganizationRule`. This
//! emulator has one caller account and models no organization membership or delegated
//! administrator (nothing in this crate records either, and `CreateAutomationRule` takes
//! the same reading), so there is no entitlement to check and the modelled
//! `AccessDeniedException` / `ForbiddenException` are unreachable from this handler: the
//! single caller stands in for the management account. A caller that must see the refusal
//! cannot get it from this emulator.
//!
//! `maxResults` is another emulator reading, not a refusal: the model bounds it at
//! `range(min: 1, max: 100)` and the clipped prose transcribed below says 1-1000, so this
//! handler follows the MODEL, which is the constraint the protocol boundary enforces.
//!
//! ---
//!
//! # Modeled contract for `ListAutomationRulePreview`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/smithy-model/com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice.json")
//! ```
//!
//! Returns a preview of the recommended actions that match your Automation rule's
//! configuration and criteria.
//!
//! ## Input
//!
//! * `criteria` (Criteria) — A set of conditions that specify which recommended action
//!   qualify for implementation. When a rule is active and a recommended action matches these
//!   criteria, Compute Optimizer implements the action at the scheduled run time.
//! * `maxResults` (Integer) — The maximum number of automation rule preview results to return
//!   in a single response. Valid range is 1-1000. (The shape itself carries
//!   `range(min: 1, max: 100)`, which is the constraint the protocol boundary enforces; see
//!   the header above.)
//! * `nextToken` (NextToken, pattern ^[A-Za-z0-9+/=]+$) — A token used for pagination to
//!   retrieve the next set of results when the response is truncated.
//! * `organizationScope` (OrganizationScope) — The organizational scope for the rule preview.
//! * `recommendedActionTypes` (RecommendedActionTypeList) **required** — The types of
//!   recommended actions to include in the preview.
//! * `ruleType` (RuleType, one of: AccountRule | OrganizationRule) **required** — The type of
//!   rule. Only the management account or a delegated administrator can set the ruleType to
//!   be OrganizationRule.
//!
//! ## Output
//!
//! * `nextToken` (NextToken, pattern ^[A-Za-z0-9+/=]+$) — A token used for pagination. If
//!   present, indicates there are more results available and can be used in subsequent
//!   requests.
//! * `previewResults` (PreviewResults) — The list of actions that would be taken based on the
//!   specified criteria.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient permissions to perform
//!   this action.
//! * `ForbiddenException` (HTTP 403) — You are not authorized to perform this action.
//! * `InternalServerException` (HTTP 500) — An internal error occurred while processing the
//!   request.
//! * `InvalidParameterValueException` (HTTP 400) — One or more parameter values are not
//!   valid.
//! * `OptInRequiredException` (HTTP 403) — The account must be opted in to Compute Optimizer
//!   Automation before performing this action.
//! * `ServiceUnavailableException` (HTTP 503) — The service is temporarily unavailable.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to request throttling.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListAutomationRulePreviewError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListAutomationRulePreviewError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.computeoptimizerautomation#ListAutomationRulePreview")]
#[derive(Debug)]
pub enum ListAutomationRulePreviewErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ForbiddenException(ForbiddenException),
    InternalServerException(InternalServerException),
    InvalidParameterValueException(InvalidParameterValueException),
    OptInRequiredException(OptInRequiredException),
    ServiceUnavailableException(ServiceUnavailableException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_automation_rule_preview::ListAutomationRulePreviewError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListAutomationRulePreviewError = crate::scaffolded::Scaffolded<ListAutomationRulePreviewErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListAutomationRulePreviewErrorModeled> for ListAutomationRulePreviewError {
    fn from(modeled: ListAutomationRulePreviewErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListAutomationRulePreview` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListAutomationRulePreview<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListAutomationRulePreview<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListAutomationRulePreview<Eng>
{
    type Input = ListAutomationRulePreviewInput;
    type Output = ListAutomationRulePreviewOutput;
    type Error = ListAutomationRulePreviewError;

    const NAME: &'static str = "ListAutomationRulePreview";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_AUTOMATION_RULE_PREVIEW_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_AUTOMATION_RULE_PREVIEW_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                ListAutomationRulePreviewErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "the service is not mounted on a state-bearing engine".into(),
                        ),
                    },
                )
                .into(),
            );
        };

        // Rules the model cannot express, so the protocol boundary let them
        // through. Each is refused with the only client-input error this
        // operation declares, worded by the service's one validation reporter
        // (`crate::state::constraint_report`), which echoes no request value.
        //
        // `recommendedActionTypes` is required and names the action types to
        // include, so an empty list selects nothing at all.
        if input.recommended_action_types.is_empty() {
            return Err(invalid_parameter(crate::state::constraint_report(
                "recommendedActionTypes",
                "Member must have length greater than or equal to 1",
            )));
        }
        // `organizationScope` names the accounts of an ORGANIZATION preview; an
        // account-scoped rule previews only the account it lives in, so a scope
        // sent with it is refused rather than ignored.
        if input.rule_type != "OrganizationRule" && input.organization_scope.is_some() {
            return Err(invalid_parameter(crate::state::constraint_report(
                "organizationScope",
                "Member is valid only for the OrganizationRule rule type",
            )));
        }
        if let Some(report) = input
            .criteria
            .as_ref()
            .and_then(crate::state::criteria_violation)
        {
            return Err(invalid_parameter(report));
        }

        // The page token carries the operation and the selection it was taken
        // with, so resuming a walk with a different scope, action-type list or
        // criteria — or with another operation's token — is refused instead of
        // answered with a page of a different result set.
        let selection_fingerprint = crate::state::fingerprint(&(
            Self::NAME,
            &input.rule_type,
            &input.organization_scope,
            &input.recommended_action_types,
            &input.criteria,
        ));
        let offset = match input.next_token.as_deref() {
            None => 0,
            Some(token) => {
                let Some(cursor) = crate::state::decode_page_token(token) else {
                    return Err(invalid_parameter(crate::state::constraint_report(
                        "nextToken",
                        "Member must be a token returned by a previous ListAutomationRulePreview \
                         call",
                    )));
                };
                if cursor.filter_fingerprint != selection_fingerprint {
                    return Err(invalid_parameter(crate::state::constraint_report(
                        "nextToken",
                        "Member must be a token returned for the same rule type, action types and \
                         criteria",
                    )));
                }
                cursor.offset
            }
        };

        // The model bounds `maxResults` at 1..=100, so a caller that sends none
        // gets the largest page it could have asked for, and the protocol
        // boundary has already refused anything outside that range. Clamped into
        // the range here as well: a zero page would return nothing while still
        // carrying a token whose offset never advances, so a walk would never
        // terminate.
        let max_page_size = usize::try_from(crate::state::DEFAULT_MAX_RESULTS).unwrap_or(1);
        let page_size = usize::try_from(
            input
                .max_results
                .unwrap_or(crate::state::DEFAULT_MAX_RESULTS),
        )
        .unwrap_or(1)
        .clamp(1, max_page_size);

        let scope = crate::state::preview_account_scope(
            &input.rule_type,
            input.organization_scope.as_ref(),
        );
        // A caller that sends no criteria constrains nothing, so every action in
        // scope carrying a requested action type is previewed.
        let criteria = input.criteria.unwrap_or_default();

        let guard = state.lock();
        // The service-wide enrollment gate, AHEAD of the inventory walk: an
        // account that is not opted in is refused the whole call rather than
        // handed an (empty) preview that tooling would then build on. See
        // `crate::state::…::opt_in_required`.
        if let Some(refusal) = guard.opt_in_required() {
            return Err(
                ListAutomationRulePreviewErrorModeled::OptInRequiredException(refusal).into(),
            );
        }
        // The selection itself — scope, requested action types, criteria — is
        // `crate::state::previewed_actions`, so this operation and
        // `ListAutomationRulePreviewSummaries` report over exactly the same set
        // of actions for the same request.
        let matching: Vec<PreviewResult> = crate::state::previewed_actions(
            &guard.recommended_actions,
            scope.as_deref(),
            &input.recommended_action_types,
            &criteria,
        )
        .skip(offset)
        .take(page_size.saturating_add(1))
        .map(crate::state::preview_result_for)
        .collect();

        // One extra result was taken: its presence is what says another page
        // exists, and the token resumes exactly where this page stopped.
        let has_more = matching.len() > page_size;
        let page: Vec<PreviewResult> = matching.into_iter().take(page_size).collect();
        let next_token = has_more.then(|| {
            crate::state::encode_page_token(&crate::state::PageCursor {
                offset: offset + page.len(),
                filter_fingerprint: selection_fingerprint,
            })
        });

        Ok(ListAutomationRulePreviewOutput {
            preview_results: Some(page),
            next_token,
        })
    }
}

fn invalid_parameter(message: String) -> ListAutomationRulePreviewError {
    ListAutomationRulePreviewErrorModeled::InvalidParameterValueException(
        InvalidParameterValueException {
            message: Some(message),
        },
    )
    .into()
}
