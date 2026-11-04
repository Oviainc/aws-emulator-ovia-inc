// SPDX-License-Identifier: CC0-1.0

//! @op ListEventTypes
//!
//! Reports the event types available for configuring notifications: the catalog
//! [`crate::state::EVENT_TYPE_CATALOG`] publishes, in its published order, honouring
//! `Filters` (OR within one filter name, AND across names), `MaxResults` and the opaque
//! `NextToken` the previous page ended with.
//!
//! The catalog is the service's own reference data — there is no API that creates an
//! event type — so this operation reports it and writes nothing.
//!
//! ---
//!
//! # Modeled contract for `ListEventTypes`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json")
//! ```
//!
//! Returns information about the event types available for configuring notifications.
//!
//! ## Input
//!
//! * `Filters` (ListEventTypesFilters) — The filters to use to return information by service
//!   or resource type.
//! * `MaxResults` (MaxResults, range 1..100) — A non-negative integer used to limit the
//!   number of returned results. The default number is 50. The maximum number of results that
//!   can be returned is 100.
//! * `NextToken` (NextToken, pattern ^[\w/+=]+$) — An enumeration token that, when provided
//!   in a request, returns the next batch of the results.
//!
//! ## Output
//!
//! * `EventTypes` (EventTypeBatch) — Information about each event, including service name,
//!   resource type, event ID, and event name.
//! * `NextToken` (NextToken, pattern ^[\w/+=]+$) — An enumeration token that can be used in a
//!   request to return the next batch of the results.
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
// `ListEventTypesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListEventTypesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.codestarnotifications#ListEventTypes")]
#[derive(Debug)]
pub enum ListEventTypesErrorModeled {
    InvalidNextTokenException(InvalidNextTokenException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_event_types::ListEventTypesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListEventTypesError = crate::scaffolded::Scaffolded<ListEventTypesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListEventTypesErrorModeled> for ListEventTypesError {
    fn from(modeled: ListEventTypesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// HTTP 400 `ValidationException`: "One or more parameter values are not valid."
fn invalid(message: impl Into<String>) -> ListEventTypesError {
    ListEventTypesErrorModeled::ValidationException(ValidationException {
        message: Some(message.into()),
    })
    .into()
}

/// The page size used when a request carries no `MaxResults`: `MaxResults`' own
/// documentation for this operation — "The default number is 50." This operation
/// publishes its own default, so it does not take the service-wide one.
const DEFAULT_PAGE_SIZE: usize = 50;

/// Does one catalogued event type satisfy one filter?
///
/// The filter names are `ListEventTypesFilterName`'s own values. The comparison ignores
/// ASCII case because `ListEventTypesFilter$Value` documents a resource type in lower
/// case ("for example, pipeline") while `EventTypeSummary$ResourceType` reports the
/// capitalised noun, so both spellings name the same type.
fn entry_satisfies(
    entry: &crate::state::EventTypeCatalogEntry,
    name: &str,
    value: &str,
) -> bool {
    match name {
        "RESOURCE_TYPE" => entry.resource_type.eq_ignore_ascii_case(value),
        "SERVICE_NAME" => entry.service_name.eq_ignore_ascii_case(value),
        // Unreachable: an unknown name is refused before this runs.
        _ => false,
    }
}

/// The `ListEventTypes` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListEventTypes<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListEventTypes<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListEventTypes<Eng>
{
    type Input = ListEventTypesInput;
    type Output = ListEventTypesOutput;
    type Error = ListEventTypesError;

    const NAME: &'static str = "ListEventTypes";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_EVENT_TYPES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_EVENT_TYPES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state as st;

        // The event-type catalog is the service's published reference data, the same
        // table `DescribeNotificationRule` resolves a rule's stored ids through: no
        // request can add to it or take from it, so this operation reads it and touches
        // no per-caller store.
        let _ = &self.engine;

        let filters = input.filters.clone().unwrap_or_default();
        // Check and collapse the request's filters ONCE: the bound on how many a request
        // may carry, the filter names this operation's own enum lists, and the
        // OR-within-a-name / AND-across-names grouping are the same for every listing
        // this service serves, so they live in `crate::state`.
        let pairs: Vec<(&str, &str)> = filters
            .iter()
            .map(|filter| (filter.name.as_str(), filter.value.as_str()))
            .collect();
        let grouped = st::grouped_filters(&pairs, LIST_EVENT_TYPES_FILTER_NAME_VALUES)
            .map_err(invalid)?;

        let start = match input.next_token.as_deref() {
            None => 0,
            Some(token) => match st::decode_page_token(token) {
                Some(position) => usize::try_from(position).unwrap_or(usize::MAX),
                None => {
                    return Err(ListEventTypesErrorModeled::InvalidNextTokenException(
                        InvalidNextTokenException {
                            message: Some(st::invalid_next_token(token)),
                        },
                    )
                    .into());
                }
            },
        };
        let page_size = input.max_results.map_or(DEFAULT_PAGE_SIZE, |max| {
            usize::try_from(max).unwrap_or(DEFAULT_PAGE_SIZE).max(1)
        });

        // The catalog's own order is the stable total order pages walk, so a token is
        // the position in it the next page resumes at.
        let matching: Vec<(usize, EventTypeSummary)> = st::EVENT_TYPE_CATALOG
            .iter()
            .enumerate()
            .filter(|(position, _)| *position >= start)
            .filter(|(_, entry)| {
                grouped.iter().all(|(name, values)| {
                    values
                        .iter()
                        .any(|value| entry_satisfies(entry, name, value))
                })
            })
            .map(|(position, entry)| {
                (
                    position,
                    EventTypeSummary {
                        event_type_id: Some(entry.event_type_id.to_string()),
                        service_name: Some(entry.service_name.to_string()),
                        resource_type: Some(entry.resource_type.to_string()),
                        event_type_name: Some(entry.event_type_name.to_string()),
                    },
                )
            })
            .collect();

        let has_more = matching.len() > page_size;
        let page: Vec<(usize, EventTypeSummary)> = matching.into_iter().take(page_size).collect();
        // Resume one position past the entry this page ended on, so the next page starts
        // exactly where this one stopped.
        let next_token = match (has_more, page.last()) {
            (true, Some((position, _))) => Some(st::encode_page_token(*position as u64 + 1)),
            _ => None,
        };

        Ok(ListEventTypesOutput {
            event_types: Some(page.into_iter().map(|(_, summary)| summary).collect()),
            next_token,
        })
    }
}
