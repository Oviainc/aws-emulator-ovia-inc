// SPDX-License-Identifier: CC0-1.0

//! @op ListTargets
//!
//! Lists the notification rule targets [`crate::state`]'s account-wide target registry
//! holds for the request's region — a target exists for the account while at least one
//! rule subscribes to its address, and carries that address, its type and its
//! `TargetStatus` — honouring `Filters` (OR within one filter name, AND across names, as
//! the member documents), `MaxResults` and the opaque `NextToken` the previous page ended
//! with.
//!
//! ---
//!
//! # Modeled contract for `ListTargets`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json")
//! ```
//!
//! Returns a list of the notification rule targets for an Amazon Web Services account.
//!
//! ## Input
//!
//! * `Filters` (ListTargetsFilters) — The filters to use to return information by service or
//!   resource type. Valid filters include target type, target address, and target status. A
//!   filter with the same name can appear more than once when used with OR statements.
//!   Filters with different names should be applied with AND statements.
//! * `MaxResults` (MaxResults, range 1..100) — A non-negative integer used to limit the
//!   number of returned results. The maximum number of results that can be returned is 100.
//! * `NextToken` (NextToken, pattern ^[\w/+=]+$) — An enumeration token that, when provided
//!   in a request, returns the next batch of the results.
//!
//! ## Output
//!
//! * `NextToken` (NextToken, pattern ^[\w/+=]+$) — An enumeration token that can be used in a
//!   request to return the next batch of results.
//! * `Targets` (TargetsBatch) — The list of notification rule targets.
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
// `ListTargetsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListTargetsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.codestarnotifications#ListTargets")]
#[derive(Debug)]
pub enum ListTargetsErrorModeled {
    InvalidNextTokenException(InvalidNextTokenException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_targets::ListTargetsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListTargetsError = crate::scaffolded::Scaffolded<ListTargetsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListTargetsErrorModeled> for ListTargetsError {
    fn from(modeled: ListTargetsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// Does the registry entry for `address` satisfy one filter? The filter names are
/// `ListTargetsFilterName`'s own values, and each one matches the member of the target
/// summary it names, exactly.
fn target_satisfies(
    address: &str,
    entry: &crate::state::TargetRegistryEntry,
    name: &str,
    value: &str,
) -> bool {
    match name {
        "TARGET_ADDRESS" => address == value,
        "TARGET_TYPE" => entry.target_type == value,
        "TARGET_STATUS" => entry.status == value,
        // Unreachable: an unknown name is refused before this runs.
        _ => false,
    }
}

/// The `ListTargets` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListTargets<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListTargets<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListTargets<Eng>
{
    type Input = ListTargetsInput;
    type Output = ListTargetsOutput;
    type Error = ListTargetsError;

    const NAME: &'static str = "ListTargets";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TARGETS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TARGETS_OUTPUT_SCHEMA;
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
        let grouped =
            st::grouped_filters(&pairs, LIST_TARGETS_FILTER_NAME_VALUES).map_err(|reason| {
                ListTargetsErrorModeled::ValidationException(ValidationException {
                    message: Some(reason),
                })
            })?;

        let start = match input.next_token.as_deref() {
            None => 0,
            Some(token) => match st::decode_page_token(token) {
                Some(position) => position,
                None => {
                    return Err(ListTargetsErrorModeled::InvalidNextTokenException(
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
            // engine there is no target registry to list, which is a "not written for
            // this engine" condition, not a client error.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "ListTargets (no state store on this engine)",
            ));
        };
        let store = state.lock();

        let matching: Vec<(u64, TargetSummary)> = store
            .targets_in_region_order(&region)
            .into_iter()
            .filter(|(_, entry)| entry.sequence >= start)
            .filter(|(address, entry)| {
                grouped.iter().all(|(name, values)| {
                    values
                        .iter()
                        .any(|value| target_satisfies(address, entry, name, value))
                })
            })
            .map(|(address, entry)| {
                (
                    entry.sequence,
                    TargetSummary {
                        target_address: Some(address.to_string()),
                        target_type: Some(entry.target_type.clone()),
                        target_status: Some(entry.status.clone()),
                    },
                )
            })
            .collect();

        let has_more = matching.len() > page_size;
        let page: Vec<(u64, TargetSummary)> = matching.into_iter().take(page_size).collect();
        // Resume one position past the target this page ended on, so the next page starts
        // exactly where this one stopped.
        let next_token = match (has_more, page.last()) {
            (true, Some((sequence, _))) => Some(st::encode_page_token(sequence + 1)),
            _ => None,
        };

        Ok(ListTargetsOutput {
            next_token,
            targets: Some(page.into_iter().map(|(_, summary)| summary).collect()),
        })
    }
}
