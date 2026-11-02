// SPDX-License-Identifier: CC0-1.0

//! @op ListRequestedServiceQuotaChangeHistoryByQuota
//!
//! Lists the stored quota increase requests for one quota, NEWEST FIRST, filtered by
//! `Status` and `QuotaRequestedAtLevel` when given, and paginated.
//!
//! The requests it reports are the ones made on a plane this endpoint serves: for a
//! quota the catalog counts account-wide that is every request of the ACCOUNT, since
//! such a quota has one value and so one request history account-wide, and for a quota
//! counted per region the requests made at this endpoint.
//!
//! * an unknown `(ServiceCode, QuotaCode)` pair is `NoSuchResourceException` (404),
//!   stating whichever half of the pair the catalog does not hold
//!   ([`crate::state::ComAmazonawsServicequotasServicequotasv20190624State::unresolved_quota_message`]);
//! * `Status` keeps the requests that now carry that `RequestStatus`;
//! * `QuotaRequestedAtLevel` keeps the requests made at that level — `ALL` keeps
//!   every one, and a caller that names none gets the member's documented default
//!   ([`crate::state::requested_at_level`]);
//! * `MaxResults` / `NextToken` page the listing, and a token this service did not
//!   issue is `InvalidPaginationTokenException` (400). `MaxResults` bounds what a
//!   CALLER may ask for, not what this listing serves when none is named: a caller
//!   that names no `MaxResults` is served the whole filtered history in one page with
//!   no `NextToken` ([`crate::state::UNASKED_PAGE_SIZE`]).
//!
//! Before answering it materialises the decisions the clock has already made due, so
//! the status a request is filtered on is the one it has at the instant of the call.
//!
//! ---
//!
//! # Modeled contract for `ListRequestedServiceQuotaChangeHistoryByQuota`
//!
//! For the FULL text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! ## Input
//!
//! * `ServiceCode` (required, len 1..63, pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$)
//! * `QuotaCode` (required, len 1..128, pattern ^[a-zA-Z][a-zA-Z0-9-]{1,128}$)
//! * `Status` (RequestStatus: PENDING | CASE_OPENED | APPROVED | DENIED |
//!   NOT_APPROVED | CASE_CLOSED | INVALID_REQUEST)
//! * `MaxResults` (range 1..100), `NextToken` (len 0..2048, pattern ^[a-zA-Z0-9/+]*={0,2}$)
//! * `QuotaRequestedAtLevel` (AppliedLevelEnum: ACCOUNT | ALL | RESOURCE)
//!
//! ## Output
//!
//! * `RequestedQuotas` (RequestedServiceQuotaChangeHistoryListDefinition), `NextToken`
//!
//! ## Errors
//!
//! * `AccessDeniedException` (403), `IllegalArgumentException` (400),
//!   `InvalidPaginationTokenException` (400), `NoSuchResourceException` (404),
//!   `ServiceException` (500), `TooManyRequestsException` (429).

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json");

/// The operation's MODELED errors.
#[smithy_macros::operation_error(
    "com.amazonaws.servicequotas#ListRequestedServiceQuotaChangeHistoryByQuota"
)]
#[derive(Debug)]
pub enum ListRequestedServiceQuotaChangeHistoryByQuotaErrorModeled {
    AccessDeniedException(AccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    InvalidPaginationTokenException(InvalidPaginationTokenException),
    NoSuchResourceException(NoSuchResourceException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type ListRequestedServiceQuotaChangeHistoryByQuotaError =
    crate::scaffolded::Scaffolded<ListRequestedServiceQuotaChangeHistoryByQuotaErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<ListRequestedServiceQuotaChangeHistoryByQuotaErrorModeled>
    for ListRequestedServiceQuotaChangeHistoryByQuotaError
{
    fn from(modeled: ListRequestedServiceQuotaChangeHistoryByQuotaErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListRequestedServiceQuotaChangeHistoryByQuota` operation.
pub struct ListRequestedServiceQuotaChangeHistoryByQuota<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListRequestedServiceQuotaChangeHistoryByQuota<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListRequestedServiceQuotaChangeHistoryByQuota<Eng>
{
    type Input = ListRequestedServiceQuotaChangeHistoryByQuotaInput;
    type Output = ListRequestedServiceQuotaChangeHistoryByQuotaOutput;
    type Error = ListRequestedServiceQuotaChangeHistoryByQuotaError;

    const NAME: &'static str = "ListRequestedServiceQuotaChangeHistoryByQuota";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_REQUESTED_SERVICE_QUOTA_CHANGE_HISTORY_BY_QUOTA_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_REQUESTED_SERVICE_QUOTA_CHANGE_HISTORY_BY_QUOTA_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(&ctx.parts().headers);
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListRequestedServiceQuotaChangeHistoryByQuotaErrorModeled::ServiceException(
                ServiceException {
                    message: Some("the Service Quotas state store is unavailable".to_string()),
                },
            )
        })?;

        let history = {
            let mut quotas_state = state.lock();
            quotas_state.settle_due_requests(::std::time::SystemTime::now());

            if quotas_state
                .default_quota(&input.service_code, &input.quota_code)
                .is_none()
            {
                // The pair names no quota, so there is no history to report. WHICH half
                // of the pair the catalog does not hold decides what the refusal states
                // — a `ServiceCode` the catalog does not publish is the absent service,
                // not a missing quota of it — and that is the crate's one rule for it
                // ([`crate::state::ComAmazonawsServicequotasServicequotasv20190624State::unresolved_quota_message`]),
                // so this listing words the fact exactly as the service-wide one does.
                return Err(
                    ListRequestedServiceQuotaChangeHistoryByQuotaErrorModeled::NoSuchResourceException(
                        NoSuchResourceException {
                            message: Some(quotas_state.unresolved_quota_message(
                                &input.service_code,
                                &input.quota_code,
                            )),
                        },
                    )
                    .into(),
                );
            }

            let requests = quotas_state.requests_for_quota(
                &region,
                &input.service_code,
                &input.quota_code,
            );
            quotas_state.history_view(
                requests,
                input.status.as_deref(),
                input.quota_requested_at_level.as_deref(),
            )
        };

        // The token names a position in the listing THIS request selected — the
        // requests for one quota this endpoint serves, narrowed by the `Status` and
        // level the caller asked for — so a token cut from another selection cannot
        // resume here. A caller that names no level is served the ACCOUNT one, so the
        // two label the same listing. The position is the order key of the record the
        // page stopped after, so a further increase requested on this quota while a
        // caller walks the history repeats no record and hides none.
        let listing = crate::state::listing_label(
            Self::NAME,
            &[
                Some(&region),
                Some(&input.service_code),
                Some(&input.quota_code),
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
            ListRequestedServiceQuotaChangeHistoryByQuotaErrorModeled::InvalidPaginationTokenException(
                InvalidPaginationTokenException {
                    message: Some(
                        "NextToken does not name a position in this listing".to_string(),
                    ),
                },
            )
        })?;

        Ok(ListRequestedServiceQuotaChangeHistoryByQuotaOutput {
            requested_quotas: Some(page.items),
            next_token: page.next_token,
        })
    }
}
