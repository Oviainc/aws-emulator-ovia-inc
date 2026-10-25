// SPDX-License-Identifier: CC0-1.0

//! @op DescribeApplicationStatusChecks
//!
//! Reports the CONFIGURATION of the application status checks the store holds
//! ([`crate::state::ComAmazonawsEc2Amazonec2State::application_status_checks`]) — "including
//! configuration details such as protocol, port, path, thresholds, and associations" —
//! narrowed by the ids, the `aggregation` filter and the `IncludeAll` choice the request
//! names, and cut into `MaxResults`-sized pages in `ApplicationStatusCheckId` order.
//!
//! A READ: nothing here writes, and no member of a reported check is derived from the clock,
//! so describing a check changes nothing a later caller sees. The per-instance RESULTS these
//! checks report are a different store, which `DescribeApplicationStatus` aggregates
//! ([`crate::state::ComAmazonawsEc2Amazonec2State::application_status`]).
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The store
//! is reached through [`crate::state::require_service_state`], the way every handler beside
//! this one reaches it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeApplicationStatusChecks`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Describes application status checks, including configuration details such as protocol,
//! port, path, thresholds, and associations. Results are paginated. Use the NextToken
//! parameter to retrieve additional results. The following rules apply: If you do not
//! specify any application status check IDs, all checks in your account are returned. Use
//! DescribeApplicationStatus to see the actual health status of instances.
//!
//! ## Input
//!
//! * `ApplicationStatusCheckIds` (ApplicationStatusCheckIdList) — The IDs of the application
//!   status checks to describe.
//! * `DryRun` (Boolean) — Checks whether you have the required permissions for the operation,
//!   without actually making the request, and provides an error response. If you have the
//!   required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `Filters` (FilterList) — The filters. aggregation – The aggregation setting. Valid
//!   values: included and excluded.
//! * `IncludeAll` (Boolean) — Specifies whether to include recently deleted application
//!   status checks that remain available during the deletion grace period. If you omit this
//!   parameter or set it to false, the response includes only active checks.
//! * `MaxResults` (DescribeApplicationStatusChecksMaxResults, range 5..100) — The maximum
//!   number of items to return for this request. To get the next page of items, make another
//!   request with the token returned in the output. For more information, see Pagination.
//! * `NextToken` (String) — The token returned from a previous paginated request. Pagination
//!   continues from the end of the items returned by the previous request.
//!
//! ## Output
//!
//! * `ApplicationStatusChecks` (ApplicationStatusCheckResponseSet) — Information about the
//!   application status checks.
//! * `NextToken` (String) — The token to include in another request to get the next page of
//!   items. This value is null when there are no more items to return.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeApplicationStatusChecksError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeApplicationStatusChecksError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#DescribeApplicationStatusChecks")]
#[derive(Debug)]
pub enum DescribeApplicationStatusChecksErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_application_status_checks::DescribeApplicationStatusChecksError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeApplicationStatusChecksError = crate::scaffolded::Scaffolded<DescribeApplicationStatusChecksErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeApplicationStatusChecksErrorModeled> for DescribeApplicationStatusChecksError {
    fn from(modeled: DescribeApplicationStatusChecksErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeApplicationStatusChecks` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeApplicationStatusChecks<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeApplicationStatusChecks<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeApplicationStatusChecks<Eng>
{
    type Input = DescribeApplicationStatusChecksInput;
    type Output = DescribeApplicationStatusChecksOutput;
    type Error = DescribeApplicationStatusChecksError;

    const NAME: &'static str = "DescribeApplicationStatusChecks";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_APPLICATION_STATUS_CHECKS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_APPLICATION_STATUS_CHECKS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::scaffolded::Scaffolded;
        use crate::state::{
            application_status_check_is_active, decode_page_token, encode_page_token,
            invalid_pagination_token_for, matches_every_filter,
            require_known_application_status_check, require_service_state, CompiledFilter,
            MatchBudget, MAX_FILTERS, MAX_FILTER_MATCH_STEPS, MAX_FILTER_VALUES,
            MAX_FILTER_VALUE_BYTES, MAX_RESOURCE_IDS,
        };

        // `ctx` carries request metadata only; this operation reads none of it — a check's
        // configuration is a fact of the check, not of the Region the request was signed
        // for. The store is reached through the engine held on `self`.
        let _ = ctx;

        // "The IDs of the application status checks to describe"
        // (`DescribeApplicationStatusChecksRequest$ApplicationStatusCheckIds`). The model
        // bounds the list nowhere, so it is bounded here before any work proportional to it
        // is done.
        let requested_ids: &[String] = input.application_status_check_ids.as_deref().unwrap_or_default();
        if requested_ids.len() > MAX_RESOURCE_IDS {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "ApplicationStatusCheckIds carries {} ids, above the maximum of \
                 {MAX_RESOURCE_IDS}",
                requested_ids.len()
            )));
        }

        // Matching cost is filters x values x held values x stored checks and the model
        // bounds none of them, so the request-controlled factors are capped here, before any
        // work proportional to them is done — and their PRODUCT is charged to one
        // [`MatchBudget`] below, which is the bound the per-factor caps cannot give.
        let supplied_filters = input.filters.as_ref().map_or(0, Vec::len);
        if supplied_filters > MAX_FILTERS {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "The request carries {supplied_filters} filters, above the maximum \
                 of {MAX_FILTERS}"
            )));
        }

        // A filter name this operation does not document is refused whether or not any check
        // would have matched it, so an unsupported filter can never read as "no results".
        let filters: &[Filter] = input.filters.as_deref().unwrap_or_default();
        let mut selectors: Vec<CompiledFilter> = Vec::with_capacity(filters.len());
        for filter in filters {
            let name = filter.name.as_deref().unwrap_or_default();
            if !APPLICATION_STATUS_CHECK_FILTERS.contains(&name) {
                return Err(Scaffolded::invalid_parameter_value(format!(
                    "The filter '{name}' is invalid"
                )));
            }
            let values = filter.values.as_deref().unwrap_or_default();
            if values.len() > MAX_FILTER_VALUES {
                return Err(Scaffolded::invalid_parameter_value(format!(
                    "The filter '{name}' carries {} values, above the maximum of \
                     {MAX_FILTER_VALUES}",
                    values.len()
                )));
            }
            // A capped NUMBER of comparisons is not capped WORK: the matcher's cost is the
            // product of the pattern's length and the value's, so each value's size is
            // bounded too, before any matching runs under the lock.
            if let Some(oversize) = values
                .iter()
                .find(|value| value.len() > MAX_FILTER_VALUE_BYTES)
            {
                return Err(Scaffolded::invalid_parameter_value(format!(
                    "A value of the filter '{name}' carries {} bytes, above the maximum of \
                     {MAX_FILTER_VALUE_BYTES}",
                    oversize.len()
                )));
            }
            selectors.push(CompiledFilter::new(name.to_string(), values.to_vec()));
        }

        // "The token returned from a previous paginated request"
        // (`DescribeApplicationStatusChecksRequest$NextToken`) — anything else is refused
        // rather than read as an id, and it is refused before any other member is looked at,
        // so whether a bogus token is caught never depends on the rest of the request.
        let start_at = match input.next_token.as_deref() {
            None => None,
            Some(token) => {
                Some(decode_page_token(token).ok_or_else(|| invalid_pagination_token_for(token))?)
            }
        };

        let state = require_service_state(&*self.engine)?;
        let store = state.lock();

        // An id that names no application status check of the account is an error, not an
        // absent row — the same answer every other EC2 describe of this crate gives for one.
        for application_status_check_id in requested_ids {
            require_known_application_status_check(&store, application_status_check_id)?;
        }

        // DryRun sits AFTER every parameter has been validated and every named check
        // resolved: the flag asks whether the request WOULD have succeeded, so one that
        // would have been refused is refused with that reason instead.
        if input.dry_run == Some(true) {
            return Err(Scaffolded::dry_run_operation());
        }

        // "Specifies whether to include recently deleted application status checks that
        // remain available during the deletion grace period. If you omit this parameter or
        // set it to false, the response includes only active checks"
        // (`DescribeApplicationStatusChecksRequest$IncludeAll`).
        let include_all = input.include_all.unwrap_or(false);

        // "If you do not specify any application status check IDs, all checks in your
        // account are returned": the ids narrow the store to those named, and every filter
        // must then hold ("If you specify multiple filters, the filters are joined with an
        // AND" — `Filter`).
        //
        // Every comparison is charged to ONE budget, so the PRODUCT of stored checks,
        // filters, values, held values and value length is bounded however the caller
        // combines them — which is also what bounds how long this holds the state lock.
        let mut budget = MatchBudget::new(MAX_FILTER_MATCH_STEPS);
        // The ids the request named, as a set: the narrowing below is then one hash lookup
        // per stored check rather than a scan of up to `MAX_RESOURCE_IDS` ids for each.
        let narrowed_to: ::std::collections::HashSet<&str> =
            requested_ids.iter().map(String::as_str).collect();
        // Each selection keeps the STORE KEY beside the check: the key is the
        // `ApplicationStatusCheckId` the store is keyed by, so paging on it cannot be
        // affected by a stored check whose own id member is absent or disagrees with its key.
        let mut selected: Vec<(&str, &ApplicationStatusCheckResponseObject)> = Vec::new();
        for (application_status_check_id, check) in &store.application_status_checks {
            if budget.exhausted() {
                break;
            }
            // Both narrowings are read off the stored check, so one the request did not ask
            // for costs no comparison at all.
            if !narrowed_to.is_empty()
                && !narrowed_to.contains(application_status_check_id.as_str())
            {
                continue;
            }
            if !include_all && !application_status_check_is_active(check) {
                continue;
            }
            if matches_every_filter(&selectors, &mut budget, &|name| filter_values(check, name)) {
                selected.push((application_status_check_id.as_str(), check));
            }
        }
        // A spent budget means the answer is INCOMPLETE, which is not something to report as
        // a result: the request asked for more filtering than one call will do, and is told
        // so rather than given a silently truncated page.
        if budget.exhausted() {
            return Err(Scaffolded::request_limit_exceeded(format!(
                "The request's filters would take more than {MAX_FILTER_MATCH_STEPS} \
                 comparisons to evaluate; narrow them or page with MaxResults"
            )));
        }

        // Page in `ApplicationStatusCheckId` order, which the cursor below names a position
        // in. The store is an ordered map keyed by that id, so the selection is already in
        // that order; the assertion states the invariant the cursor depends on at the point of
        // use rather than re-sorting what the container already guarantees.
        debug_assert!(
            selected.is_sorted_by_key(|(application_status_check_id, _)| {
                *application_status_check_id
            }),
            "the check store must iterate in ApplicationStatusCheckId order"
        );

        let from = match start_at.as_deref() {
            None => 0,
            Some(cursor) => selected
                .partition_point(|(application_status_check_id, _)| {
                    *application_status_check_id < cursor
                }),
        };
        let page = selected.get(from..).unwrap_or_default();

        // "The maximum number of items to return for this request"
        // (`DescribeApplicationStatusChecksRequest$MaxResults`), whose modelled `@range` of
        // 5..100 the boundary validator has already enforced — so the only value that cannot
        // convert is one it would have refused. A request that names no page size gets the
        // largest page the model publishes, so the `NextToken` path below behaves the same
        // whether or not a page size was asked for.
        let limit = input
            .max_results
            .and_then(|max_results| usize::try_from(max_results).ok())
            .unwrap_or(MAX_CHECKS_PER_PAGE)
            .clamp(1, MAX_CHECKS_PER_PAGE);

        // "The token to include in another request to get the next page of items. This value
        // is null when there are no more items to return"
        // (`DescribeApplicationStatusChecksResult$NextToken`): a token only when a check
        // follows this page, naming the store key that page starts at.
        let next_token = page
            .get(limit)
            .map(|(application_status_check_id, _)| encode_page_token(application_status_check_id));

        Ok(DescribeApplicationStatusChecksOutput {
            application_status_checks: Some(
                page.iter()
                    .take(limit)
                    .map(|(_, check)| (*check).clone())
                    .collect(),
            ),
            next_token,
        })
    }
}

/// The most checks this operation reports in one page: the top of the modelled `@range` on
/// `DescribeApplicationStatusChecksMaxResults` (5..100,
/// `smithy_query(query="operation",operation="DescribeApplicationStatusChecks")`), which is
/// therefore also the page a request naming no `MaxResults` gets.
const MAX_CHECKS_PER_PAGE: usize = 100;

/// The filter names this operation documents, from
/// `DescribeApplicationStatusChecksRequest$Filters`:
///
/// ```text
/// smithy_query(query="doc",shape="DescribeApplicationStatusChecksRequest",member="Filters",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
/// ```
///
/// It documents this one and no `tag:` family, so a tag filter is as undocumented here as
/// any other name — unlike `DescribeVpcs`, whose own doc lists one. Every entry must be
/// answered by [`filter_values`].
const APPLICATION_STATUS_CHECK_FILTERS: [&str; 1] = ["aggregation"];

/// The values the EC2 filter `name` compares against one application status check — the
/// per-operation half of [`crate::state::matches_every_filter`], which holds the AND/OR
/// evaluation every filtered describe of this crate shares.
///
/// A filter naming a member this check does not carry compares against NO values, so it
/// selects nothing — the answer for `aggregation` on a check that names no aggregation
/// setting. `name` is one of [`APPLICATION_STATUS_CHECK_FILTERS`].
fn filter_values<'a>(
    check: &'a ApplicationStatusCheckResponseObject,
    name: &str,
) -> Vec<::std::borrow::Cow<'a, str>> {
    use ::std::borrow::Cow;

    match name {
        // "`aggregation` – The aggregation setting. Valid values: `included` and
        // `excluded`" — compared against the value the check holds, whichever word that is.
        "aggregation" => check
            .aggregation
            .as_deref()
            .map(Cow::Borrowed)
            .into_iter()
            .collect(),
        // Unreachable: the handler has already refused every other name. An undefined name
        // compares against nothing, so it could never widen a result even if that check were
        // bypassed.
        _ => Vec::new(),
    }
}
