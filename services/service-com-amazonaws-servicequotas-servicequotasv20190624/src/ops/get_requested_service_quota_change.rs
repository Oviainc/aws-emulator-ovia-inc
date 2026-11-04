// SPDX-License-Identifier: CC0-1.0

//! @op GetRequestedServiceQuotaChange
//!
//! Returns the stored quota increase request with the given `RequestId`, exactly as
//! `RequestServiceQuotaIncrease` recorded it — same `Id`, `ServiceCode`,
//! `QuotaCode`, `DesiredValue` and timestamps — with the status it now has.
//!
//! Before answering it materialises the steps the clock has already made due, so a
//! request read straight after it was made reports `PENDING`, one read while the
//! support case that processes it is open reports `CASE_OPENED` with that case's
//! `CaseId`, and one read past its decision reports the status it settled to. Nothing
//! else moves a status: reading does not decide a request.
//!
//! A request is read back on the plane it was made on: one for a quota the catalog
//! counts account-wide is one request for the ACCOUNT, so every regional endpoint
//! reports it — by the `Id` any endpoint handed out — while one for a quota counted per
//! region is read at the endpoint it was made at. An id no request this endpoint
//! resolves carries is `NoSuchResourceException` (404).
//!
//! ---
//!
//! # Modeled contract for `GetRequestedServiceQuotaChange`
//!
//! For the FULL text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! ## Input
//!
//! * `RequestId` (required, len 1..128, pattern ^[0-9a-zA-Z][a-zA-Z0-9-]{1,128}$)
//!
//! ## Output
//!
//! * `RequestedQuota` (RequestedServiceQuotaChange)
//!
//! ## Errors
//!
//! * `AccessDeniedException` (403), `IllegalArgumentException` (400),
//!   `NoSuchResourceException` (404), `ServiceException` (500),
//!   `TooManyRequestsException` (429).

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json");

/// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.servicequotas#GetRequestedServiceQuotaChange")]
#[derive(Debug)]
pub enum GetRequestedServiceQuotaChangeErrorModeled {
    AccessDeniedException(AccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    NoSuchResourceException(NoSuchResourceException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type GetRequestedServiceQuotaChangeError =
    crate::scaffolded::Scaffolded<GetRequestedServiceQuotaChangeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<GetRequestedServiceQuotaChangeErrorModeled> for GetRequestedServiceQuotaChangeError {
    fn from(modeled: GetRequestedServiceQuotaChangeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetRequestedServiceQuotaChange` operation.
pub struct GetRequestedServiceQuotaChange<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetRequestedServiceQuotaChange<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetRequestedServiceQuotaChange<Eng>
{
    type Input = GetRequestedServiceQuotaChangeInput;
    type Output = GetRequestedServiceQuotaChangeOutput;
    type Error = GetRequestedServiceQuotaChangeError;

    const NAME: &'static str = "GetRequestedServiceQuotaChange";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_REQUESTED_SERVICE_QUOTA_CHANGE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_REQUESTED_SERVICE_QUOTA_CHANGE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(&ctx.parts().headers);
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetRequestedServiceQuotaChangeErrorModeled::ServiceException(ServiceException {
                message: Some("the Service Quotas state store is unavailable".to_string()),
            })
        })?;

        let mut quotas_state = state.lock();
        quotas_state.settle_due_requests(::std::time::SystemTime::now());

        let Some(request) = quotas_state.request(&region, &input.request_id) else {
            return Err(
                GetRequestedServiceQuotaChangeErrorModeled::NoSuchResourceException(
                    NoSuchResourceException {
                        message: Some(
                            crate::service::refusals::NO_SUCH_QUOTA_REQUEST_MESSAGE.to_string(),
                        ),
                    },
                )
                .into(),
            );
        };
        let view = quotas_state.request_view(request);

        Ok(GetRequestedServiceQuotaChangeOutput {
            requested_quota: Some(view),
        })
    }
}
