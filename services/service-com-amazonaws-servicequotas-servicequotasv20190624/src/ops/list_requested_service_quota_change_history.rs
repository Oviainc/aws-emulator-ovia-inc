// SPDX-License-Identifier: CC0-1.0

//! @op ListRequestedServiceQuotaChangeHistory
//!
//! Lists the stored quota increase requests of the ACCOUNT, NEWEST FIRST, across every
//! service or narrowed to one `ServiceCode`, filtered by `Status` and
//! `QuotaRequestedAtLevel` when given, and paginated.
//!
//! This is the service-wide counterpart of
//! [`super::list_requested_service_quota_change_history_by_quota`], which lists one
//! quota's requests: the same records, the same order and the same filters, with the
//! quota narrowing dropped and `ServiceCode` optional. Both report the requests made
//! on a plane this endpoint serves: every request of the ACCOUNT for a quota the
//! catalog counts account-wide, and this endpoint's own for a quota counted per region.
//!
//! * `ServiceCode` is OPTIONAL here, so a caller that names none is served every
//!   service's requests; a code the catalog does not know is `NoSuchResourceException`
//!   (404 modelled, put on the wire as the client-fault 400 this awsJson front end
//!   answers a modelled client error on);
//! * `Status` keeps the requests that now carry that `RequestStatus`;
//! * `QuotaRequestedAtLevel` keeps the requests made at that level — `ALL` keeps every
//!   one, and a caller that names none gets the member's documented default
//!   ([`crate::state::requested_at_level`]);
//! * only requests created inside the documented
//!   [`crate::state::REQUEST_HISTORY_WINDOW`] are reported;
//! * `MaxResults` / `NextToken` page the listing, and a token this service did not
//!   issue for THIS listing is `InvalidPaginationTokenException` (400). `MaxResults`
//!   bounds what a CALLER may ask for, not what this listing serves when none is
//!   named: a caller that names no `MaxResults` is served the whole filtered history
//!   in one page with no `NextToken` ([`crate::state::UNASKED_PAGE_SIZE`]).
//!
//! Before answering it materialises the decisions the clock has already made due, so
//! the status a request is filtered on is the one it has at the instant of the call.
//!
//! ---
//!
//! # Modeled contract for `ListRequestedServiceQuotaChangeHistory`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! Retrieves the quota increase requests for the specified Amazon Web Services service.
//! Filter responses to return quota requests at either the account level, resource level,
//! or all levels. Responses include any open or closed requests within 90 days.
//!
//! ## Input
//!
//! * `MaxResults` (MaxResults, range 1..100) — Specifies the maximum number of results that
//!   you want included on each page of the response. If you do not include this parameter, it
//!   defaults to a value appropriate to the operation. If additional items exist beyond those
//!   included in the current response, the NextToken response element is present and has a
//!   value (is not null). Include that value as the NextToken request parameter in the next
//!   call to the operation to get the next part of the results. An API operation can return
//!   fewer results than the maximum even when there are more results available. You should
//!   check NextToken after every operation to ensure that you receive all of the results.
//! * `NextToken` (NextToken, len 0..2048, pattern ^[a-zA-Z0-9/+]*={0,2}$) — Specifies a value
//!   for receiving additional results after you receive a NextToken response in a previous
//!   request. A NextToken response indicates that more output is available. Set this
//!   parameter to the value of the previous call's NextToken response to indicate where the
//!   output should continue from.
//! * `QuotaRequestedAtLevel` (AppliedLevelEnum, one of: ACCOUNT | ALL | RESOURCE) — Filters
//!   the response to return quota requests for the ACCOUNT, RESOURCE, or ALL levels. ACCOUNT
//!   is the default.
//! * `ServiceCode` (ServiceCode, len 1..63, pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$) — Specifies
//!   the service identifier. To find the service code value for an Amazon Web Services
//!   service, use the ListServices operation.
//! * `Status` (RequestStatus, one of: APPROVED | CASE_CLOSED | CASE_OPENED | DENIED |
//!   INVALID_REQUEST | NOT_APPROVED | PENDING) — Specifies that you want to filter the
//!   results to only the requests with the matching status.
//!
//! ## Output
//!
//! * `NextToken` (NextToken, len 0..2048, pattern ^[a-zA-Z0-9/+]*={0,2}$) — If present,
//!   indicates that more output is available than is included in the current response. Use
//!   this value in the NextToken request parameter in a subsequent call to the operation to
//!   get the next part of the output. You should repeat this until the NextToken response
//!   element comes back as null.
//! * `RequestedQuotas` (RequestedServiceQuotaChangeHistoryListDefinition) — Information about
//!   the quota increase requests.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient permission to perform
//!   this action.
//! * `IllegalArgumentException` (HTTP 400) — Invalid input was provided.
//! * `InvalidPaginationTokenException` (HTTP 400) — Invalid input was provided.
//! * `NoSuchResourceException` (HTTP 404) — The specified resource does not exist.
//! * `ServiceException` (HTTP 500) — Something went wrong.
//! * `TooManyRequestsException` (HTTP 429) — Due to throttling, the request was denied. Slow
//!   down the rate of request calls, or request an increase for this quota.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListRequestedServiceQuotaChangeHistoryError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListRequestedServiceQuotaChangeHistoryError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.servicequotas#ListRequestedServiceQuotaChangeHistory")]
#[derive(Debug)]
pub enum ListRequestedServiceQuotaChangeHistoryErrorModeled {
    AccessDeniedException(AccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    InvalidPaginationTokenException(InvalidPaginationTokenException),
    NoSuchResourceException(NoSuchResourceException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_requested_service_quota_change_history::ListRequestedServiceQuotaChangeHistoryError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListRequestedServiceQuotaChangeHistoryError = crate::scaffolded::Scaffolded<ListRequestedServiceQuotaChangeHistoryErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListRequestedServiceQuotaChangeHistoryErrorModeled> for ListRequestedServiceQuotaChangeHistoryError {
    fn from(modeled: ListRequestedServiceQuotaChangeHistoryErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListRequestedServiceQuotaChangeHistory` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListRequestedServiceQuotaChangeHistory<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListRequestedServiceQuotaChangeHistory<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListRequestedServiceQuotaChangeHistory<Eng>
{
    type Input = ListRequestedServiceQuotaChangeHistoryInput;
    type Output = ListRequestedServiceQuotaChangeHistoryOutput;
    type Error = ListRequestedServiceQuotaChangeHistoryError;

    const NAME: &'static str = "ListRequestedServiceQuotaChangeHistory";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_REQUESTED_SERVICE_QUOTA_CHANGE_HISTORY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_REQUESTED_SERVICE_QUOTA_CHANGE_HISTORY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(&ctx.parts().headers);
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListRequestedServiceQuotaChangeHistoryErrorModeled::ServiceException(ServiceException {
                message: Some("the Service Quotas state store is unavailable".to_string()),
            })
        })?;

        let history = {
            let now = ::std::time::SystemTime::now();
            let mut quotas_state = state.lock();
            quotas_state.settle_due_requests(now);

            // A `ServiceCode` the catalog does not publish names no service, so the
            // listing is a lookup failure rather than an empty page. A caller that
            // names none asks for every service's requests.
            if let Some(service_code) = input.service_code.as_deref() {
                if !quotas_state.knows_service(service_code) {
                    return Err(
                        ListRequestedServiceQuotaChangeHistoryErrorModeled::NoSuchResourceException(
                            NoSuchResourceException {
                                message: Some(
                                    crate::service::refusals::SERVICE_NOT_AVAILABLE_MESSAGE
                                        .to_string(),
                                ),
                            },
                        )
                        .into(),
                    );
                }
            }

            let requests =
                quotas_state.requests_in_history(&region, input.service_code.as_deref(), now);
            quotas_state.history_view(
                requests,
                input.status.as_deref(),
                input.quota_requested_at_level.as_deref(),
            )
        };

        // The token names a position in the listing THIS request selected — the
        // requests this endpoint serves, narrowed by the `ServiceCode`, `Status` and
        // level the caller asked for — so a token cut from another selection cannot
        // resume here. A caller that names no level is served the ACCOUNT one, so the
        // two label the same listing. The position is the order key of the record the
        // page stopped after, so a request made while a caller walks the history —
        // which the listing reports FIRST, being the newest — repeats no record and
        // hides none.
        let listing = crate::state::listing_label(
            Self::NAME,
            &[
                Some(&region),
                input.service_code.as_deref(),
                input.status.as_deref(),
                Some(crate::state::level_filter(
                    input.quota_requested_at_level.as_deref(),
                )),
            ],
        );
        let page = crate::state::paginate_after(
            &listing,
            history,
            input.max_results,
            input.next_token.as_deref(),
        )
        .map_err(|_| {
            ListRequestedServiceQuotaChangeHistoryErrorModeled::InvalidPaginationTokenException(
                InvalidPaginationTokenException {
                    message: Some(
                        "NextToken does not name a position in this listing".to_string(),
                    ),
                },
            )
        })?;

        Ok(ListRequestedServiceQuotaChangeHistoryOutput {
            requested_quotas: Some(page.items),
            next_token: page.next_token,
        })
    }
}
