// SPDX-License-Identifier: CC0-1.0

//! @op GetAWSDefaultServiceQuota
//!
//! Returns the AWS DEFAULT value of one `(ServiceCode, QuotaCode)` pair: the value
//! the quota has before any increase, read straight out of the seeded default map in
//! [`crate::state`].
//!
//! That map is seeded once and has no writer, so the value this operation reports is
//! the same before and after an increase is requested and applied — which is the
//! whole reason the service has both this operation and `GetServiceQuota`. Unlike
//! `GetServiceQuota` it reports no `QuotaAppliedAtLevel`: nothing has been applied.
//!
//! An unknown pair is `NoSuchResourceException`.
//!
//! Its `QuotaArn` is the AWS-DEFAULT form, whose account segment is EMPTY
//! (`arn:aws:servicequotas:<region>::<serviceCode>/<quotaCode>`): a default quota is
//! AWS's published value and belongs to no account, while the applied quota
//! `GetServiceQuota` reports is the account's and names it.
//!
//! ---
//!
//! # Modeled contract for `GetAWSDefaultServiceQuota`
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
//!
//! ## Output
//!
//! * `Quota` (ServiceQuota)
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
#[smithy_macros::operation_error("com.amazonaws.servicequotas#GetAWSDefaultServiceQuota")]
#[derive(Debug)]
pub enum GetAWSDefaultServiceQuotaErrorModeled {
    AccessDeniedException(AccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    NoSuchResourceException(NoSuchResourceException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type GetAWSDefaultServiceQuotaError =
    crate::scaffolded::Scaffolded<GetAWSDefaultServiceQuotaErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<GetAWSDefaultServiceQuotaErrorModeled> for GetAWSDefaultServiceQuotaError {
    fn from(modeled: GetAWSDefaultServiceQuotaErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetAWSDefaultServiceQuota` operation.
pub struct GetAWSDefaultServiceQuota<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetAWSDefaultServiceQuota<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetAWSDefaultServiceQuota<Eng>
{
    type Input = GetAWSDefaultServiceQuotaInput;
    type Output = GetAWSDefaultServiceQuotaOutput;
    type Error = GetAWSDefaultServiceQuotaError;

    const NAME: &'static str = "GetAWSDefaultServiceQuota";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_AWS_DEFAULT_SERVICE_QUOTA_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_AWS_DEFAULT_SERVICE_QUOTA_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(&ctx.parts().headers);
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetAWSDefaultServiceQuotaErrorModeled::ServiceException(ServiceException {
                message: Some("the Service Quotas state store is unavailable".to_string()),
            })
        })?;

        // A default quota never changes, so nothing is settled here: this operation
        // only reads the seeded map.
        let quotas_state = state.lock();
        let quota = quotas_state.default_quota_view(&region, &input.service_code, &input.quota_code);

        let Some(quota) = quota else {
            // A pair the catalog does not hold is the crate's one sentence for that
            // failed lookup, whichever half of the pair is missing — the service's own
            // fixed text, carrying nothing of the request.
            return Err(
                GetAWSDefaultServiceQuotaErrorModeled::NoSuchResourceException(
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

        Ok(GetAWSDefaultServiceQuotaOutput {
            quota: Some(quota),
        })
    }
}
