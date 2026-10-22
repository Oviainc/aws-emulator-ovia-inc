// SPDX-License-Identifier: CC0-1.0

//! @op GetAssociationForServiceQuotaTemplate
//!
//! Reports whether the account's quota request template is associated with its
//! organization.
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
//! third answer and there is no association status to report — see
//! [`crate::state::template_refusal`] for why. Reporting `DISASSOCIATED` would be the
//! WRONG answer: it would say the account has a template plane it has merely turned
//! off. Nothing is read, and no other Service Quotas call is affected.
//!
//! ---
//!
//! # Modeled contract for `GetAssociationForServiceQuotaTemplate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! Retrieves the status of the association for the quota request template.
//!
//! ## Output
//!
//! * `ServiceQuotaTemplateAssociationStatus` (ServiceQuotaTemplateAssociationStatus, one of:
//!   ASSOCIATED | DISASSOCIATED) — The association status. If the status is ASSOCIATED, the
//!   quota increase requests in the template are automatically applied to new Amazon Web
//!   Services accounts in your organization.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient permission to perform
//!   this action.
//! * `AWSServiceAccessNotEnabledException` (HTTP 403) — The action you attempted is not
//!   allowed unless Service Access with Service Quotas is enabled in your organization.
//! * `DependencyAccessDeniedException` (HTTP 403) — You can't perform this action because a
//!   dependency does not have access.
//! * `NoAvailableOrganizationException` (HTTP 403) — The Amazon Web Services account making
//!   this call is not a member of an organization.
//! * `ServiceException` (HTTP 500) — Something went wrong.
//! * `ServiceQuotaTemplateNotInUseException` (HTTP 400) — The quota request template is not
//!   associated with your organization.
//! * `TemplatesNotAvailableInRegionException` (HTTP 404) — The Service Quotas template is not
//!   available in this Amazon Web Services Region.
//! * `TooManyRequestsException` (HTTP 429) — Due to throttling, the request was denied. Slow
//!   down the rate of request calls, or request an increase for this quota.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetAssociationForServiceQuotaTemplateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetAssociationForServiceQuotaTemplateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.servicequotas#GetAssociationForServiceQuotaTemplate")]
#[derive(Debug)]
pub enum GetAssociationForServiceQuotaTemplateErrorModeled {
    AccessDeniedException(AccessDeniedException),
    AWSServiceAccessNotEnabledException(AWSServiceAccessNotEnabledException),
    DependencyAccessDeniedException(DependencyAccessDeniedException),
    NoAvailableOrganizationException(NoAvailableOrganizationException),
    ServiceException(ServiceException),
    ServiceQuotaTemplateNotInUseException(ServiceQuotaTemplateNotInUseException),
    TemplatesNotAvailableInRegionException(TemplatesNotAvailableInRegionException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_association_for_service_quota_template::GetAssociationForServiceQuotaTemplateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetAssociationForServiceQuotaTemplateError = crate::scaffolded::Scaffolded<GetAssociationForServiceQuotaTemplateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetAssociationForServiceQuotaTemplateErrorModeled> for GetAssociationForServiceQuotaTemplateError {
    fn from(modeled: GetAssociationForServiceQuotaTemplateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetAssociationForServiceQuotaTemplate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetAssociationForServiceQuotaTemplate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetAssociationForServiceQuotaTemplate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetAssociationForServiceQuotaTemplate<Eng>
{
    type Input = GetAssociationForServiceQuotaTemplateInput;
    type Output = GetAssociationForServiceQuotaTemplateOutput;
    type Error = GetAssociationForServiceQuotaTemplateError;

    const NAME: &'static str = "GetAssociationForServiceQuotaTemplate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ASSOCIATION_FOR_SERVICE_QUOTA_TEMPLATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ASSOCIATION_FOR_SERVICE_QUOTA_TEMPLATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The template gate refuses this call before anything is read, so the
        // request's members and this service's store are both untouched; they are
        // bound here because the struct owns the engine either way.
        let _ = (&input, &self.engine);
        let region = crate::state::request_region(&ctx.parts().headers);
        Err(template_refusal(&region).into())
    }
}

/// The gate's refusal, in this operation's own modelled error enum.
fn template_refusal(region: &str) -> GetAssociationForServiceQuotaTemplateErrorModeled {
    match crate::state::template_refusal(region) {
        crate::state::TemplateRefusal::NotAvailableInRegion => {
            GetAssociationForServiceQuotaTemplateErrorModeled::TemplatesNotAvailableInRegionException(
                TemplatesNotAvailableInRegionException {
                    message: Some(format!(
                        "The Service Quotas template is not available in Region '{region}'."
                    )),
                },
            )
        }
        crate::state::TemplateRefusal::NoAvailableOrganization => {
            GetAssociationForServiceQuotaTemplateErrorModeled::NoAvailableOrganizationException(
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
