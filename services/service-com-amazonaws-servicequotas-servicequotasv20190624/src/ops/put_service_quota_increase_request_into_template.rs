// SPDX-License-Identifier: CC0-1.0

//! @op PutServiceQuotaIncreaseRequestIntoTemplate
//!
//! Adds a quota increase request to the account's quota request template.
//!
//! Every quota request TEMPLATE operation passes one gate first
//! ([`crate::state::template_gate`]), because a template is an ORGANIZATION-wide object:
//! a call from outside the plane's Region, from an account in no AWS Organization, or
//! from one whose organization is not in all-features mode is refused there.
//!
//! Past the gate the entry is STORED, keyed by the `(ServiceCode, AwsRegion, QuotaCode)`
//! triple that identifies it, and the stored entry is what is returned — so
//! `GetServiceQuotaIncreaseRequestFromTemplate` and
//! `ListServiceQuotaIncreaseRequestsInTemplate` read back the same record, and a second
//! put for the same triple replaces its value rather than adding a second entry for one
//! quota in one Region.
//!
//! Refusals of the request itself:
//!
//! * an unknown `(ServiceCode, QuotaCode)` pair -> `NoSuchResourceException` (404);
//! * an `AwsRegion` that is not an Amazon Web Services Region ->
//!   `IllegalArgumentException` (400): the template applies its entry in that Region;
//! * a quota AWS publishes as not adjustable -> `IllegalArgumentException` (400), since
//!   "You can add a quota increase request for any adjustable quota to your template";
//! * a `DesiredValue` that is not above the quota's AWS DEFAULT value ->
//!   `IllegalArgumentException` (400): this entry is an INCREASE, and it is applied to
//!   accounts that are created in the organization, which hold the default;
//! * a template already holding [`crate::state::MAX_TEMPLATE_ENTRIES`] entries ->
//!   `QuotaExceededException` (409).
//!
//! ---
//!
//! # Modeled contract for `PutServiceQuotaIncreaseRequestIntoTemplate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! Adds a quota increase request to your quota request template.
//!
//! ## Input
//!
//! * `AwsRegion` (AwsRegion, len 1..64, pattern ^[a-zA-Z][a-zA-Z0-9-]{1,128}$) **required** —
//!   Specifies the Amazon Web Services Region to which the template applies.
//! * `DesiredValue` (QuotaValue, range 0..10000000000) **required** — Specifies the new,
//!   increased value for the quota.
//! * `QuotaCode` (QuotaCode, len 1..128, pattern ^[a-zA-Z][a-zA-Z0-9-]{1,128}$) **required**
//!   — Specifies the quota identifier. To find the quota code for a specific quota, use the
//!   ListServiceQuotas operation, and look for the QuotaCode response in the output for the
//!   quota you want.
//! * `ServiceCode` (ServiceCode, len 1..63, pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$)
//!   **required** — Specifies the service identifier. To find the service code value for an
//!   Amazon Web Services service, use the ListServices operation.
//!
//! ## Output
//!
//! * `ServiceQuotaIncreaseRequestInTemplate` (ServiceQuotaIncreaseRequestInTemplate) —
//!   Information about the quota increase request.
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
//! * `QuotaExceededException` (HTTP 409) — You have exceeded your service quota. To perform
//!   the requested action, remove some of the relevant resources, or use Service Quotas to
//!   request a service quota increase.
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
// `PutServiceQuotaIncreaseRequestIntoTemplateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PutServiceQuotaIncreaseRequestIntoTemplateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.servicequotas#PutServiceQuotaIncreaseRequestIntoTemplate")]
#[derive(Debug)]
pub enum PutServiceQuotaIncreaseRequestIntoTemplateErrorModeled {
    AccessDeniedException(AccessDeniedException),
    AWSServiceAccessNotEnabledException(AWSServiceAccessNotEnabledException),
    DependencyAccessDeniedException(DependencyAccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    NoAvailableOrganizationException(NoAvailableOrganizationException),
    NoSuchResourceException(NoSuchResourceException),
    QuotaExceededException(QuotaExceededException),
    ServiceException(ServiceException),
    TemplatesNotAvailableInRegionException(TemplatesNotAvailableInRegionException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_service_quota_increase_request_into_template::PutServiceQuotaIncreaseRequestIntoTemplateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutServiceQuotaIncreaseRequestIntoTemplateError = crate::scaffolded::Scaffolded<PutServiceQuotaIncreaseRequestIntoTemplateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutServiceQuotaIncreaseRequestIntoTemplateErrorModeled> for PutServiceQuotaIncreaseRequestIntoTemplateError {
    fn from(modeled: PutServiceQuotaIncreaseRequestIntoTemplateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutServiceQuotaIncreaseRequestIntoTemplate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutServiceQuotaIncreaseRequestIntoTemplate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutServiceQuotaIncreaseRequestIntoTemplate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutServiceQuotaIncreaseRequestIntoTemplate<Eng>
{
    type Input = PutServiceQuotaIncreaseRequestIntoTemplateInput;
    type Output = PutServiceQuotaIncreaseRequestIntoTemplateOutput;
    type Error = PutServiceQuotaIncreaseRequestIntoTemplateError;

    const NAME: &'static str = "PutServiceQuotaIncreaseRequestIntoTemplate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_SERVICE_QUOTA_INCREASE_REQUEST_INTO_TEMPLATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_SERVICE_QUOTA_INCREASE_REQUEST_INTO_TEMPLATE_OUTPUT_SCHEMA;
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
            PutServiceQuotaIncreaseRequestIntoTemplateErrorModeled::ServiceException(
                ServiceException {
                    message: Some("the Service Quotas state store is unavailable".to_string()),
                },
            )
        })?;

        let mut quotas_state = state.lock();
        let Some(default) = quotas_state.default_quota(&input.service_code, &input.quota_code)
        else {
            // A pair the catalog does not hold is the crate's one sentence for that
            // failed lookup, whichever half of the pair is missing — the service's own
            // fixed text, carrying nothing of the request.
            return Err(
                PutServiceQuotaIncreaseRequestIntoTemplateErrorModeled::NoSuchResourceException(
                    NoSuchResourceException {
                        message: Some(
                            crate::service::refusals::QUOTA_AND_SERVICE_ABSENT_MESSAGE
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        };
        let adjustable = default.adjustable;
        let default_value = default.value;

        // The entry names the Region its increase is applied in, so that Region has to be
        // one: `AwsRegion`'s modelled pattern admits any word, and an entry for a Region
        // that does not exist could never be applied.
        if !crate::state::KNOWN_REGIONS.contains(&input.aws_region.as_str()) {
            return Err(
                PutServiceQuotaIncreaseRequestIntoTemplateErrorModeled::IllegalArgumentException(
                    IllegalArgumentException {
                        message: Some(format!(
                            "AwsRegion '{}' is not an Amazon Web Services Region",
                            input.aws_region
                        )),
                    },
                )
                .into(),
            );
        }

        if !adjustable {
            // The crate's one sentence for that fact, shared with the write that asks
            // for an increase outright: fixed text, carrying neither code the request
            // named.
            return Err(
                PutServiceQuotaIncreaseRequestIntoTemplateErrorModeled::IllegalArgumentException(
                    IllegalArgumentException {
                        message: Some(
                            crate::service::refusals::QUOTA_NOT_ADJUSTABLE_MESSAGE.to_string(),
                        ),
                    },
                )
                .into(),
            );
        }

        // The entry is applied to accounts created in the organization, which hold the
        // AWS default for the quota, so the increase is measured against that default —
        // and refused through the crate's one rule for a value that is not an increase,
        // which puts the default bound first and states each bound's own sentence.
        if input.desired_value <= default_value {
            return Err(
                PutServiceQuotaIncreaseRequestIntoTemplateErrorModeled::IllegalArgumentException(
                    IllegalArgumentException {
                        message: Some(crate::service::refusals::not_an_increase_message(
                            input.desired_value,
                            default_value,
                        )),
                    },
                )
                .into(),
            );
        }

        let key = crate::state::TemplateKey {
            service_code: input.service_code.clone(),
            aws_region: input.aws_region.clone(),
            quota_code: input.quota_code.clone(),
        };
        if !quotas_state.put_template_entry(key.clone(), input.desired_value) {
            return Err(
                PutServiceQuotaIncreaseRequestIntoTemplateErrorModeled::QuotaExceededException(
                    QuotaExceededException {
                        message: Some(format!(
                            "the quota request template already holds the maximum of {} quota \
                             increase requests",
                            crate::state::MAX_TEMPLATE_ENTRIES
                        )),
                    },
                )
                .into(),
            );
        }

        // The ENTRY AS STORED is what the caller is answered with, so the response and
        // every later read of it are the same record.
        Ok(PutServiceQuotaIncreaseRequestIntoTemplateOutput {
            service_quota_increase_request_in_template: quotas_state.template_entry(&key),
        })
    }
}

/// The gate's refusal, in this operation's own modelled error enum.
fn template_refusal(
    refusal: crate::state::TemplateRefusal,
    region: &str,
) -> PutServiceQuotaIncreaseRequestIntoTemplateErrorModeled {
    match refusal {
        crate::state::TemplateRefusal::NotAvailableInRegion => {
            PutServiceQuotaIncreaseRequestIntoTemplateErrorModeled::TemplatesNotAvailableInRegionException(
                TemplatesNotAvailableInRegionException {
                    message: Some(crate::state::template_refusal_message(refusal, region)),
                },
            )
        }
        crate::state::TemplateRefusal::NoAvailableOrganization => {
            PutServiceQuotaIncreaseRequestIntoTemplateErrorModeled::NoAvailableOrganizationException(
                NoAvailableOrganizationException {
                    message: Some(crate::state::template_refusal_message(refusal, region)),
                },
            )
        }
        // This operation does not model the all-features condition; what it models is the
        // Service Access that condition denies.
        crate::state::TemplateRefusal::OrganizationNotInAllFeaturesMode => {
            PutServiceQuotaIncreaseRequestIntoTemplateErrorModeled::AWSServiceAccessNotEnabledException(
                AWSServiceAccessNotEnabledException {
                    message: Some(crate::state::service_access_not_enabled_message()),
                },
            )
        }
    }
}
