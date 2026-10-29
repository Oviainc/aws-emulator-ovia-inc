// SPDX-License-Identifier: CC0-1.0

//! @op DisassociateServiceQuotaTemplate
//!
//! Disables the account's quota request template, so it is no longer applied to new
//! member accounts.
//!
//! Every quota request TEMPLATE operation passes one gate first
//! ([`crate::state::template_gate`]), because a template is an ORGANIZATION-wide object:
//! a call from outside the plane's Region, from an account in no AWS Organization, or
//! from one whose organization is not in all-features mode is refused there.
//!
//! Past the gate this ENDS the association, so the template's entries stop being applied
//! to accounts created afterwards and `GetAssociationForServiceQuotaTemplate` reports
//! `DISASSOCIATED`. The entries themselves are kept — "Disabling a quota request template
//! does not apply its quota increase requests", and it does not delete them either, so
//! associating again applies the same ones. A template that is not associated (never was,
//! or already disassociated) has no association to end, which is the modelled
//! `ServiceQuotaTemplateNotInUseException`.
//!
//! ---
//!
//! # Modeled contract for `DisassociateServiceQuotaTemplate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! Disables your quota request template. After a template is disabled, the quota increase
//! requests in the template are not applied to new Amazon Web Services accounts in your
//! organization. Disabling a quota request template does not apply its quota increase
//! requests.
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
// `DisassociateServiceQuotaTemplateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DisassociateServiceQuotaTemplateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.servicequotas#DisassociateServiceQuotaTemplate")]
#[derive(Debug)]
pub enum DisassociateServiceQuotaTemplateErrorModeled {
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
/// (`crate::ops::disassociate_service_quota_template::DisassociateServiceQuotaTemplateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DisassociateServiceQuotaTemplateError = crate::scaffolded::Scaffolded<DisassociateServiceQuotaTemplateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DisassociateServiceQuotaTemplateErrorModeled> for DisassociateServiceQuotaTemplateError {
    fn from(modeled: DisassociateServiceQuotaTemplateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DisassociateServiceQuotaTemplate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DisassociateServiceQuotaTemplate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DisassociateServiceQuotaTemplate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DisassociateServiceQuotaTemplate<Eng>
{
    type Input = DisassociateServiceQuotaTemplateInput;
    type Output = DisassociateServiceQuotaTemplateOutput;
    type Error = DisassociateServiceQuotaTemplateError;

    const NAME: &'static str = "DisassociateServiceQuotaTemplate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DISASSOCIATE_SERVICE_QUOTA_TEMPLATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DISASSOCIATE_SERVICE_QUOTA_TEMPLATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // This operation takes no members: the template it disables is the caller's own.
        let _ = &input;
        let region = crate::state::request_region(&ctx.parts().headers);
        // The gate reads the caller's organization from the service that owns it, so it
        // is evaluated before this service's own store is locked.
        if let Err(refusal) = crate::state::template_gate(ctx, &region).await {
            return Err(template_refusal(refusal, &region).into());
        }
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DisassociateServiceQuotaTemplateErrorModeled::ServiceException(ServiceException {
                message: Some("the Service Quotas state store is unavailable".to_string()),
            })
        })?;

        if !state.lock().disassociate_template() {
            return Err(
                DisassociateServiceQuotaTemplateErrorModeled::ServiceQuotaTemplateNotInUseException(
                    ServiceQuotaTemplateNotInUseException {
                        message: Some(
                            "The quota request template is not associated with your organization."
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        }
        Ok(DisassociateServiceQuotaTemplateOutput {})
    }
}

/// The gate's refusal, in this operation's own modelled error enum.
fn template_refusal(
    refusal: crate::state::TemplateRefusal,
    region: &str,
) -> DisassociateServiceQuotaTemplateErrorModeled {
    match refusal {
        crate::state::TemplateRefusal::NotAvailableInRegion => {
            DisassociateServiceQuotaTemplateErrorModeled::TemplatesNotAvailableInRegionException(
                TemplatesNotAvailableInRegionException {
                    message: Some(crate::state::template_refusal_message(refusal, region)),
                },
            )
        }
        crate::state::TemplateRefusal::NoAvailableOrganization => {
            DisassociateServiceQuotaTemplateErrorModeled::NoAvailableOrganizationException(
                NoAvailableOrganizationException {
                    message: Some(crate::state::template_refusal_message(refusal, region)),
                },
            )
        }
        // This operation does not model the all-features condition; what it models is the
        // Service Access that condition denies.
        crate::state::TemplateRefusal::OrganizationNotInAllFeaturesMode => {
            DisassociateServiceQuotaTemplateErrorModeled::AWSServiceAccessNotEnabledException(
                AWSServiceAccessNotEnabledException {
                    message: Some(crate::state::service_access_not_enabled_message()),
                },
            )
        }
    }
}
