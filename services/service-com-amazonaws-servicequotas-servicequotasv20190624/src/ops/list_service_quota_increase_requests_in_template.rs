// SPDX-License-Identifier: CC0-1.0

//! @op ListServiceQuotaIncreaseRequestsInTemplate
//!
//! Lists the quota increase requests in the account's quota request template.
//!
//! Every quota request TEMPLATE operation passes one gate first
//! ([`crate::state::template_gate`]), because a template is an ORGANIZATION-wide object:
//! a call from outside the plane's Region, from an account in no AWS Organization, or
//! from one whose organization is not in all-features mode is refused there.
//!
//! Past the gate this ENUMERATES the stored entries and writes nothing: narrowed to the
//! optional `ServiceCode` and `AwsRegion` the call names (a call that names neither gets
//! the whole template), in the entries' own stable order, and paginated by `MaxResults` /
//! `NextToken`. The token names the POSITION in that order — the key of the entry the
//! page stopped after — so an entry put into or deleted from the template around a page
//! boundary leaves a walk in progress reporting every other entry exactly once. A token
//! that does not name a position in THIS listing is the modelled
//! `IllegalArgumentException` — the refusal this operation carries for a bad input, since
//! it models no pagination-token error of its own.
//!
//! ---
//!
//! # Modeled contract for `ListServiceQuotaIncreaseRequestsInTemplate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! Lists the quota increase requests in the specified quota request template.
//!
//! ## Input
//!
//! * `AwsRegion` (AwsRegion, len 1..64, pattern ^[a-zA-Z][a-zA-Z0-9-]{1,128}$) — Specifies
//!   the Amazon Web Services Region for which you made the request.
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
//! * `ServiceCode` (ServiceCode, len 1..63, pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$) — Specifies
//!   the service identifier. To find the service code value for an Amazon Web Services
//!   service, use the ListServices operation.
//!
//! ## Output
//!
//! * `NextToken` (NextToken, len 0..2048, pattern ^[a-zA-Z0-9/+]*={0,2}$) — If present,
//!   indicates that more output is available than is included in the current response. Use
//!   this value in the NextToken request parameter in a subsequent call to the operation to
//!   get the next part of the output. You should repeat this until the NextToken response
//!   element comes back as null.
//! * `ServiceQuotaIncreaseRequestInTemplateList` (ServiceQuotaIncreaseRequestInTemplateList)
//!   — Information about the quota increase requests.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient permission to perform
//!   this action.
//! * `AWSServiceAccessNotEnabledException` (HTTP 403) — The action you attempted is not
//!   allowed unless Service Access with Service Quotas is enabled in your organization.
//! * `DependencyAccessDeniedException` (HTTP 403) — You can't perform this action because a
//!   dependency does not have access.
//! * `IllegalArgumentException` (HTTP 400) — Invalid input was provided.
//! * `NoAvailableOrganizationException` (HTTP 403) — The Amazon Web Services account making
//!   this call is not a member of an organization.
//! * `ServiceException` (HTTP 500) — Something went wrong.
//! * `TemplatesNotAvailableInRegionException` (HTTP 404) — The Service Quotas template is not
//!   available in this Amazon Web Services Region.
//! * `TooManyRequestsException` (HTTP 429) — Due to throttling, the request was denied. Slow
//!   down the rate of request calls, or request an increase for this quota.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListServiceQuotaIncreaseRequestsInTemplateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListServiceQuotaIncreaseRequestsInTemplateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.servicequotas#ListServiceQuotaIncreaseRequestsInTemplate")]
#[derive(Debug)]
pub enum ListServiceQuotaIncreaseRequestsInTemplateErrorModeled {
    AccessDeniedException(AccessDeniedException),
    AWSServiceAccessNotEnabledException(AWSServiceAccessNotEnabledException),
    DependencyAccessDeniedException(DependencyAccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    NoAvailableOrganizationException(NoAvailableOrganizationException),
    ServiceException(ServiceException),
    TemplatesNotAvailableInRegionException(TemplatesNotAvailableInRegionException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_service_quota_increase_requests_in_template::ListServiceQuotaIncreaseRequestsInTemplateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListServiceQuotaIncreaseRequestsInTemplateError = crate::scaffolded::Scaffolded<ListServiceQuotaIncreaseRequestsInTemplateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListServiceQuotaIncreaseRequestsInTemplateErrorModeled> for ListServiceQuotaIncreaseRequestsInTemplateError {
    fn from(modeled: ListServiceQuotaIncreaseRequestsInTemplateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListServiceQuotaIncreaseRequestsInTemplate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListServiceQuotaIncreaseRequestsInTemplate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListServiceQuotaIncreaseRequestsInTemplate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListServiceQuotaIncreaseRequestsInTemplate<Eng>
{
    type Input = ListServiceQuotaIncreaseRequestsInTemplateInput;
    type Output = ListServiceQuotaIncreaseRequestsInTemplateOutput;
    type Error = ListServiceQuotaIncreaseRequestsInTemplateError;

    const NAME: &'static str = "ListServiceQuotaIncreaseRequestsInTemplate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_SERVICE_QUOTA_INCREASE_REQUESTS_IN_TEMPLATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_SERVICE_QUOTA_INCREASE_REQUESTS_IN_TEMPLATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(&ctx.parts().headers);
        // The gate reads the caller's organization from the service that owns it, so it
        // is evaluated before this service's own store is locked.
        if let Err(refusal) = crate::state::template_gate(ctx, &region).await {
            return Err(template_refusal(refusal, &region).into());
        }
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListServiceQuotaIncreaseRequestsInTemplateErrorModeled::ServiceException(
                ServiceException {
                    message: Some("the Service Quotas state store is unavailable".to_string()),
                },
            )
        })?;

        // A pure read: it reports the stored entries and writes nothing. Each entry
        // comes paired with the order key that names its position in the template's
        // own key order, which is what the page token carries.
        let entries = state.lock().template_entries(
            input.service_code.as_deref(),
            input.aws_region.as_deref(),
        );
        // The token names a position in the listing the FILTERS selected, so both of
        // them label it: a token issued for one selection is refused by another instead
        // of resuming at an unrelated position.
        let label = crate::state::listing_label(
            Self::NAME,
            &[input.service_code.as_deref(), input.aws_region.as_deref()],
        );
        let page = crate::state::paginate_after(
            &label,
            entries,
            input.max_results,
            input.next_token.as_deref(),
        )
        .map_err(|_| {
            ListServiceQuotaIncreaseRequestsInTemplateErrorModeled::IllegalArgumentException(
                IllegalArgumentException {
                    message: Some(
                        "NextToken does not name a position in this listing".to_string(),
                    ),
                },
            )
        })?;
        Ok(ListServiceQuotaIncreaseRequestsInTemplateOutput {
            service_quota_increase_request_in_template_list: Some(page.items),
            next_token: page.next_token,
        })
    }
}

/// The gate's refusal, in this operation's own modelled error enum.
fn template_refusal(
    refusal: crate::state::TemplateRefusal,
    region: &str,
) -> ListServiceQuotaIncreaseRequestsInTemplateErrorModeled {
    match refusal {
        crate::state::TemplateRefusal::NotAvailableInRegion => {
            ListServiceQuotaIncreaseRequestsInTemplateErrorModeled::TemplatesNotAvailableInRegionException(
                TemplatesNotAvailableInRegionException {
                    message: Some(crate::state::template_refusal_message(refusal, region)),
                },
            )
        }
        crate::state::TemplateRefusal::NoAvailableOrganization => {
            ListServiceQuotaIncreaseRequestsInTemplateErrorModeled::NoAvailableOrganizationException(
                NoAvailableOrganizationException {
                    message: Some(crate::state::template_refusal_message(refusal, region)),
                },
            )
        }
        // This operation does not model the all-features condition; what it models is the
        // Service Access that condition denies.
        crate::state::TemplateRefusal::OrganizationNotInAllFeaturesMode => {
            ListServiceQuotaIncreaseRequestsInTemplateErrorModeled::AWSServiceAccessNotEnabledException(
                AWSServiceAccessNotEnabledException {
                    message: Some(crate::state::service_access_not_enabled_message()),
                },
            )
        }
    }
}
