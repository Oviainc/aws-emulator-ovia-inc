// SPDX-License-Identifier: CC0-1.0

//! @op DeleteServiceQuotaIncreaseRequestFromTemplate
//!
//! Deletes one quota increase request from the account's quota request template.
//!
//! Every quota request TEMPLATE operation passes one gate first
//! ([`crate::state::template_refusal`]), and that gate is what a caller observes here:
//!
//! * a call made at a Region the template plane is not served from ->
//!   `TemplatesNotAvailableInRegionException`;
//! * a call made at its home Region by an account that belongs to no AWS Organization
//!   -> `NoAvailableOrganizationException`.
//!
//! Both are client faults, so this awsJson front end serves them on HTTP 400 whatever
//! `@httpError` the shape declares ([`crate::scaffolded::wire_http_status`]).
//!
//! The account this emulator serves is a single STANDALONE account, so the gate has no
//! third answer and there is no template entry to delete — see
//! [`crate::state::template_refusal`] for why. Nothing is written, and no other Service
//! Quotas call is affected.
//!
//! ---
//!
//! # Modeled contract for `DeleteServiceQuotaIncreaseRequestFromTemplate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! Deletes the quota increase request for the specified quota from your quota request
//! template.
//!
//! ## Input
//!
//! * `AwsRegion` (AwsRegion, len 1..64, pattern ^[a-zA-Z][a-zA-Z0-9-]{1,128}$) **required** —
//!   Specifies the Amazon Web Services Region for which the request was made.
//! * `QuotaCode` (QuotaCode, len 1..128, pattern ^[a-zA-Z][a-zA-Z0-9-]{1,128}$) **required**
//!   — Specifies the quota identifier. To find the quota code for a specific quota, use the
//!   ListServiceQuotas operation, and look for the QuotaCode response in the output for the
//!   quota you want.
//! * `ServiceCode` (ServiceCode, len 1..63, pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$)
//!   **required** — Specifies the service identifier. To find the service code value for an
//!   Amazon Web Services service, use the ListServices operation.
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
//! * `NoSuchResourceException` (HTTP 404) — The specified resource does not exist.
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
// `DeleteServiceQuotaIncreaseRequestFromTemplateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteServiceQuotaIncreaseRequestFromTemplateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.servicequotas#DeleteServiceQuotaIncreaseRequestFromTemplate")]
#[derive(Debug)]
pub enum DeleteServiceQuotaIncreaseRequestFromTemplateErrorModeled {
    AccessDeniedException(AccessDeniedException),
    AWSServiceAccessNotEnabledException(AWSServiceAccessNotEnabledException),
    DependencyAccessDeniedException(DependencyAccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    NoAvailableOrganizationException(NoAvailableOrganizationException),
    NoSuchResourceException(NoSuchResourceException),
    ServiceException(ServiceException),
    TemplatesNotAvailableInRegionException(TemplatesNotAvailableInRegionException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_service_quota_increase_request_from_template::DeleteServiceQuotaIncreaseRequestFromTemplateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteServiceQuotaIncreaseRequestFromTemplateError = crate::scaffolded::Scaffolded<DeleteServiceQuotaIncreaseRequestFromTemplateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteServiceQuotaIncreaseRequestFromTemplateErrorModeled> for DeleteServiceQuotaIncreaseRequestFromTemplateError {
    fn from(modeled: DeleteServiceQuotaIncreaseRequestFromTemplateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteServiceQuotaIncreaseRequestFromTemplate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteServiceQuotaIncreaseRequestFromTemplate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteServiceQuotaIncreaseRequestFromTemplate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteServiceQuotaIncreaseRequestFromTemplate<Eng>
{
    type Input = DeleteServiceQuotaIncreaseRequestFromTemplateInput;
    type Output = DeleteServiceQuotaIncreaseRequestFromTemplateOutput;
    type Error = DeleteServiceQuotaIncreaseRequestFromTemplateError;

    const NAME: &'static str = "DeleteServiceQuotaIncreaseRequestFromTemplate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_SERVICE_QUOTA_INCREASE_REQUEST_FROM_TEMPLATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_SERVICE_QUOTA_INCREASE_REQUEST_FROM_TEMPLATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The template gate refuses this call before anything is read or written, so
        // the request's members and this service's store are both untouched; they are
        // bound here because the struct owns the engine either way.
        let _ = (&input, &self.engine);
        let region = crate::state::request_region(&ctx.parts().headers);
        Err(template_refusal(&region).into())
    }
}

/// The gate's refusal, in this operation's own modelled error enum.
fn template_refusal(region: &str) -> DeleteServiceQuotaIncreaseRequestFromTemplateErrorModeled {
    match crate::state::template_refusal(region) {
        crate::state::TemplateRefusal::NotAvailableInRegion => {
            DeleteServiceQuotaIncreaseRequestFromTemplateErrorModeled::TemplatesNotAvailableInRegionException(
                TemplatesNotAvailableInRegionException {
                    message: Some(format!(
                        "The Service Quotas template is not available in Region '{region}'."
                    )),
                },
            )
        }
        crate::state::TemplateRefusal::NoAvailableOrganization => {
            DeleteServiceQuotaIncreaseRequestFromTemplateErrorModeled::NoAvailableOrganizationException(
                NoAvailableOrganizationException {
                    message: Some(
                        "The Amazon Web Services account making this call is not a member of an \
                         organization."
                            .to_string(),
                    ),
                },
            )
        }
    }
}
