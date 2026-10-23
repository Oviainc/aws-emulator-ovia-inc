// SPDX-License-Identifier: CC0-1.0

//! @op ListAutomationRulePreviewSummaries
//! Summarises the recommended actions an Automation rule's preview selects: the
//! SAME set `ListAutomationRulePreview` reports one by one (the actions this service
//! holds for the accounts the rule type and `organizationScope` cover, restricted to
//! the requested `recommendedActionTypes` and to the `criteria` conditions), reduced
//! to one summary per distinct `recommendedActionType` among them, each carrying how
//! many of those actions it groups and the sum of their estimated monthly savings.
//!
//! The grouping dimension is the EMULATOR's reading: `PreviewResultSummary.key` is
//! documented only as "The key identifier for this preview result summary" and this
//! shape, unlike `AutomationEventSummary`, carries no `dimensions` member naming it
//! (`smithy_query(query="shape",shape="PreviewResultSummary,AutomationEventSummary,SummaryDimension")`).
//! `recommendedActionType` is the one dimension a preview request itself selects on —
//! it is required, and `RecommendedActionType` enumerates its values — so a summary
//! key is the action type of the actions it totals, and a summary appears only for an
//! action type the selection actually holds (the summary reports what matched, so an
//! action type nothing matched contributes no group). Groups come back in the order
//! the store first holds an action of each type, the same record order the
//! unsummarised preview pages over, cut into `maxResults`-sized pages.
//!
//! `@readonly`, so this handler writes NOTHING: the inventory it reports over is the
//! analysis output Compute Optimizer derives from observed resources
//! (`crate::state::RecommendedActionStore`), and a page token carries its own cursor.
//!
//! The refusals below are the EMULATOR's choice rather than modelled constraints, each
//! reported with the only client-input error this operation declares
//! (`InvalidParameterValueException`), and each the same reading its unsummarised
//! sibling takes — one request cannot be valid for a preview and invalid for that
//! preview's totals:
//!
//! * an `organizationScope` sent with a rule type other than `OrganizationRule`;
//! * an EMPTY `recommendedActionTypes`, which selects no action type at all;
//! * a `criteria` this service cannot evaluate (a condition with no `comparison`, no
//!   `values`, an operator from the wrong family, or a `resourceTag` condition with no
//!   `key`);
//! * a `nextToken` that does not decode, or that was taken with a different rule type,
//!   scope, action-type list or criteria — or from another operation, including
//!   `ListAutomationRulePreview`, whose pages are single actions rather than groups.
//!
//! NOT enforced here, deliberately: `ruleType`'s prose says only the management account
//! or a delegated administrator may ask for `OrganizationRule`. This emulator has one
//! caller account and models no organization membership or delegated administrator, so
//! there is no entitlement to check and the modelled `AccessDeniedException` /
//! `ForbiddenException` are unreachable from this handler, exactly as in
//! `ListAutomationRulePreview`.
//!
//! `maxResults` is an emulator reading, not a refusal: the model bounds it at
//! `range(min: 1, max: 100)` while the clipped prose transcribed below says 1-1000, so
//! this handler follows the MODEL, which is the constraint the protocol boundary
//! enforces.
//!
//! ---
//!
//! # Modeled contract for `ListAutomationRulePreviewSummaries`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/smithy-model/com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice.json")
//! ```
//!
//! Returns a summary of the recommended actions that match your rule preview configuration
//! and criteria.
//!
//! ## Input
//!
//! * `criteria` (Criteria) — A set of conditions that specify which recommended action
//!   qualify for implementation. When a rule is active and a recommended action matches these
//!   criteria, Compute Optimizer implements the action at the scheduled run time. You can
//!   specify up to 20 conditions per filter criteria and 20 values per condition.
//! * `maxResults` (Integer) — The maximum number of automation rule preview summaries to
//!   return in a single response. Valid range is 1-1000.
//! * `nextToken` (NextToken, pattern ^[A-Za-z0-9+/=]+$) — A token used for pagination to
//!   retrieve the next set of results when the response is truncated.
//! * `organizationScope` (OrganizationScope) — The organizational scope for the rule preview.
//! * `recommendedActionTypes` (RecommendedActionTypeList) **required** — The types of
//!   recommended actions to include in the preview.
//! * `ruleType` (RuleType, one of: AccountRule | OrganizationRule) **required** — The type of
//!   rule.
//!
//! ## Output
//!
//! * `nextToken` (NextToken, pattern ^[A-Za-z0-9+/=]+$) — A token used for pagination. If
//!   present, indicates there are more results available and can be used in subsequent
//!   requests.
//! * `previewResultSummaries` (PreviewResultSummaries) — The list of automation rule preview
//!   summaries that match the specified criteria.
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
// `ListAutomationRulePreviewSummariesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListAutomationRulePreviewSummariesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.computeoptimizerautomation#ListAutomationRulePreviewSummaries")]
#[derive(Debug)]
pub enum ListAutomationRulePreviewSummariesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ForbiddenException(ForbiddenException),
    InternalServerException(InternalServerException),
    InvalidParameterValueException(InvalidParameterValueException),
    OptInRequiredException(OptInRequiredException),
    ServiceUnavailableException(ServiceUnavailableException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_automation_rule_preview_summaries::ListAutomationRulePreviewSummariesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListAutomationRulePreviewSummariesError = crate::scaffolded::Scaffolded<ListAutomationRulePreviewSummariesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListAutomationRulePreviewSummariesErrorModeled> for ListAutomationRulePreviewSummariesError {
    fn from(modeled: ListAutomationRulePreviewSummariesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListAutomationRulePreviewSummaries` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListAutomationRulePreviewSummaries<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListAutomationRulePreviewSummaries<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListAutomationRulePreviewSummaries<Eng>
{
    type Input = ListAutomationRulePreviewSummariesInput;
    type Output = ListAutomationRulePreviewSummariesOutput;
    type Error = ListAutomationRulePreviewSummariesError;

    const NAME: &'static str = "ListAutomationRulePreviewSummaries";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_AUTOMATION_RULE_PREVIEW_SUMMARIES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_AUTOMATION_RULE_PREVIEW_SUMMARIES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                ListAutomationRulePreviewSummariesErrorModeled::InternalServerException(
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
        // include, so an empty list selects nothing at all — and a summary of
        // nothing would read as "your rule matches no resource".
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
                        "Member must be a token returned by a previous \
                         ListAutomationRulePreviewSummaries call",
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
        // scope carrying a requested action type is summarised.
        let criteria = input.criteria.unwrap_or_default();

        let guard = state.lock();
        // The service-wide enrollment gate, AHEAD of the inventory walk: an
        // account that is not opted in is refused the whole call rather than
        // handed an (empty) summary that tooling would then build on. See
        // `crate::state::…::opt_in_required`.
        if let Some(refusal) = guard.opt_in_required() {
            return Err(
                ListAutomationRulePreviewSummariesErrorModeled::OptInRequiredException(refusal)
                    .into(),
            );
        }

        // One group per `recommendedActionType` the selection holds, in the
        // order the store first holds an action of that type: the selected
        // actions are what gets summarised, never the request's action-type
        // list, so a type nothing matched is no group.
        //
        // The groups are kept as a SEQUENCE keyed by the action type, so a
        // group's key and its tally are one value: there is no second lookup
        // that could miss and report a plausible zero total. The number of
        // groups is bounded by the `RecommendedActionType` value set, so finding
        // one by key is a walk over a handful of entries.
        let mut grouped: Vec<(String, Group)> = Vec::new();
        for action in crate::state::previewed_actions(
            &guard.recommended_actions,
            scope.as_deref(),
            &input.recommended_action_types,
            &criteria,
        ) {
            // `previewed_actions` keeps only an action whose type is one of the
            // requested ones, so every action reaching here carries one.
            let Some(action_type) = action.recommended_action_type.as_deref() else {
                continue;
            };
            let index = match grouped.iter().position(|(key, _)| key == action_type) {
                Some(index) => index,
                None => {
                    grouped.push((action_type.to_string(), Group::default()));
                    grouped.len().saturating_sub(1)
                }
            };
            if let Some((_, group)) = grouped.get_mut(index) {
                group.count = group.count.saturating_add(1);
                if let Some(savings) = action.estimated_monthly_savings.as_ref() {
                    group.estimates.push(savings.clone());
                }
            }
        }

        // One extra group is taken: its presence is what says another page
        // exists, and the token resumes exactly where this page stopped.
        let taken: Vec<(String, Group)> = grouped
            .into_iter()
            .skip(offset)
            .take(page_size.saturating_add(1))
            .collect();
        let has_more = taken.len() > page_size;
        let page: Vec<PreviewResultSummary> = taken
            .into_iter()
            .take(page_size)
            .map(|(key, group)| PreviewResultSummary {
                key,
                total: RulePreviewTotal {
                    recommended_action_count: group.count,
                    estimated_monthly_savings: crate::state::total_savings(group.estimates.iter()),
                },
            })
            .collect();
        let next_token = has_more.then(|| {
            crate::state::encode_page_token(&crate::state::PageCursor {
                offset: offset + page.len(),
                filter_fingerprint: selection_fingerprint,
            })
        });

        Ok(ListAutomationRulePreviewSummariesOutput {
            preview_result_summaries: Some(page),
            next_token,
        })
    }
}

/// What one group of selected actions accumulates while the inventory is walked:
/// how many actions it holds and the estimates among them, which
/// [`crate::state::total_savings`] reduces to the group's
/// `estimatedMonthlySavings`. An action Compute Optimizer has not estimated
/// savings for is still one action of the group.
#[derive(Debug, Default)]
struct Group {
    count: i32,
    estimates: Vec<EstimatedMonthlySavings>,
}

fn invalid_parameter(message: String) -> ListAutomationRulePreviewSummariesError {
    ListAutomationRulePreviewSummariesErrorModeled::InvalidParameterValueException(
        InvalidParameterValueException {
            message: Some(message),
        },
    )
    .into()
}
