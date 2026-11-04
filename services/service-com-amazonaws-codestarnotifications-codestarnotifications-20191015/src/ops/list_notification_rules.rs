// SPDX-License-Identifier: CC0-1.0

//! @op ListNotificationRules
//!
//! Lists the notification rules [`crate::state`] holds for the request's region, in the
//! account's creation order, honouring `Filters` (OR within one filter name, AND across names, as the
//! member documents), `MaxResults` and the opaque `NextToken` the previous page ended
//! with.
//!
//! ---
//!
//! # Modeled contract for `ListNotificationRules`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json")
//! ```
//!
//! Returns a list of the notification rules for an Amazon Web Services account.
//!
//! ## Input
//!
//! * `Filters` (ListNotificationRulesFilters) — The filters to use to return information by
//!   service or resource type. For valid values, see ListNotificationRulesFilter. A filter
//!   with the same name can appear more than once when used with OR statements. Filters with
//!   different names should be applied with AND statements.
//! * `MaxResults` (MaxResults, range 1..100) — A non-negative integer used to limit the
//!   number of returned results. The maximum number of results that can be returned is 100.
//! * `NextToken` (NextToken, pattern ^[\w/+=]+$) — An enumeration token that, when provided
//!   in a request, returns the next batch of the results.
//!
//! ## Output
//!
//! * `NextToken` (NextToken, pattern ^[\w/+=]+$) — An enumeration token that can be used in a
//!   request to return the next batch of the results.
//! * `NotificationRules` (NotificationRuleBatch) — The list of notification rules for the
//!   Amazon Web Services account, by Amazon Resource Name (ARN) and ID.
//!
//! ## Errors
//!
//! * `InvalidNextTokenException` (HTTP 400) — The value for the enumeration token used in the
//!   request to return the next batch of the results is not valid.
//! * `ValidationException` (HTTP 400) — One or more parameter values are not valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListNotificationRulesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListNotificationRulesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.codestarnotifications#ListNotificationRules")]
#[derive(Debug)]
pub enum ListNotificationRulesErrorModeled {
    InvalidNextTokenException(InvalidNextTokenException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_notification_rules::ListNotificationRulesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListNotificationRulesError = crate::scaffolded::Scaffolded<ListNotificationRulesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListNotificationRulesErrorModeled> for ListNotificationRulesError {
    fn from(modeled: ListNotificationRulesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// Does `rule` satisfy one filter? The filter names are
/// `ListNotificationRulesFilterName`'s own values.
fn rule_satisfies(rule: &crate::state::NotificationRule, name: &str, value: &str) -> bool {
    match name {
        "CREATED_BY" => rule.created_by == value,
        "EVENT_TYPE_ID" => rule.event_type_ids.iter().any(|id| id == value),
        "RESOURCE" => rule.resource == value,
        "TARGET_ADDRESS" => rule
            .targets
            .iter()
            .any(|target| target.target_address == value),
        // Unreachable: an unknown name is refused before this runs.
        _ => false,
    }
}

/// The `ListNotificationRules` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListNotificationRules<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListNotificationRules<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListNotificationRules<Eng>
{
    type Input = ListNotificationRulesInput;
    type Output = ListNotificationRulesOutput;
    type Error = ListNotificationRulesError;

    const NAME: &'static str = "ListNotificationRules";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_NOTIFICATION_RULES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_NOTIFICATION_RULES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state as st;

        let region = st::request_region(ctx.parts());
        let filters = input.filters.clone().unwrap_or_default();
        // Check and collapse the request's filters ONCE, before the store lock is taken:
        // the bound on how many a request may carry, the filter names this operation's
        // own enum lists, and the OR-within-a-name / AND-across-names grouping
        // (`Filters`' own documentation) are the same for every listing this service
        // serves, so they live in `crate::state`.
        let pairs: Vec<(&str, &str)> = filters
            .iter()
            .map(|filter| (filter.name.as_str(), filter.value.as_str()))
            .collect();
        let grouped = st::grouped_filters(&pairs, LIST_NOTIFICATION_RULES_FILTER_NAME_VALUES)
            .map_err(|reason| {
                ListNotificationRulesErrorModeled::ValidationException(ValidationException {
                    message: Some(reason),
                })
            })?;

        let start = match input.next_token.as_deref() {
            None => 0,
            Some(token) => match st::decode_page_token(token) {
                Some(position) => position,
                None => {
                    return Err(ListNotificationRulesErrorModeled::InvalidNextTokenException(
                        InvalidNextTokenException {
                            message: Some(st::invalid_next_token(token)),
                        },
                    )
                    .into());
                }
            },
        };
        let page_size = st::page_size(input.max_results);

        let Some(state) = st::service_state(&*self.engine) else {
            // This service's store is the shared `InMemoryEngine`. Mounted on any other
            // engine there is no notification-rule store to list, which is a "not
            // written for this engine" condition, not a client error.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "ListNotificationRules (no state store on this engine)",
            ));
        };
        let store = state.lock();

        let matching: Vec<(u64, NotificationRuleSummary)> = store
            .rules_in_region_order(&region)
            .into_iter()
            .filter(|rule| rule.sequence >= start)
            .filter(|rule| {
                grouped.iter().all(|(name, values)| {
                    values
                        .iter()
                        .any(|value| rule_satisfies(rule, name, value))
                })
            })
            .map(|rule| {
                (
                    rule.sequence,
                    NotificationRuleSummary {
                        id: Some(rule.id.clone()),
                        arn: Some(rule.arn.clone()),
                    },
                )
            })
            .collect();

        let has_more = matching.len() > page_size;
        let page: Vec<(u64, NotificationRuleSummary)> =
            matching.into_iter().take(page_size).collect();
        // Resume one position past the rule this page ended on, so the next page starts
        // exactly where this one stopped.
        let next_token = match (has_more, page.last()) {
            (true, Some((sequence, _))) => Some(st::encode_page_token(sequence + 1)),
            _ => None,
        };

        Ok(ListNotificationRulesOutput {
            next_token,
            notification_rules: Some(page.into_iter().map(|(_, summary)| summary).collect()),
        })
    }
}
