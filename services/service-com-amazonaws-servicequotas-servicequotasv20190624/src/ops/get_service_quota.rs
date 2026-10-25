// SPDX-License-Identifier: CC0-1.0

//! @op GetServiceQuota
//!
//! Returns the quota APPLIED to one `(ServiceCode, QuotaCode)` pair: the AWS default
//! with any value an approved increase has put in force laid over it. It is the same
//! record `ListServiceQuotas` lists for that pair, built by the same
//! [`crate::state`] view, so the two agree field for field.
//!
//! * an unknown pair is `NoSuchResourceException`;
//! * a `ContextId` for a quota that is only adjustable at the account level is
//!   `IllegalArgumentException` — there is no resource context to read a value for.
//!
//! A `ContextId` on a quota AWS publishes as adjustable per resource SELECTS that
//! resource's plane: the answer is the value in force for that resource, reported at
//! the `RESOURCE` level with the `QuotaContext` describing it. With no `ContextId` the
//! account-wide record comes back, which is the level this operation defaults to.
//!
//! Before answering it materialises the increase decisions the clock has already made
//! due, so the value is the one in force at the instant of the call.
//!
//! ---
//!
//! # Modeled contract for `GetServiceQuota`
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
//! * `ContextId` (QuotaContextId) — the resource the quota applies to.
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
#[smithy_macros::operation_error("com.amazonaws.servicequotas#GetServiceQuota")]
#[derive(Debug)]
pub enum GetServiceQuotaErrorModeled {
    AccessDeniedException(AccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    NoSuchResourceException(NoSuchResourceException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type GetServiceQuotaError = crate::scaffolded::Scaffolded<GetServiceQuotaErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<GetServiceQuotaErrorModeled> for GetServiceQuotaError {
    fn from(modeled: GetServiceQuotaErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetServiceQuota` operation.
pub struct GetServiceQuota<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetServiceQuota<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetServiceQuota<Eng>
{
    type Input = GetServiceQuotaInput;
    type Output = GetServiceQuotaOutput;
    type Error = GetServiceQuotaError;

    const NAME: &'static str = "GetServiceQuota";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_SERVICE_QUOTA_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_SERVICE_QUOTA_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(&ctx.parts().headers);
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetServiceQuotaErrorModeled::ServiceException(ServiceException {
                message: Some("the Service Quotas state store is unavailable".to_string()),
            })
        })?;

        let mut quotas_state = state.lock();
        quotas_state.settle_due_requests(&region, ::std::time::SystemTime::now());

        let Some(default) = quotas_state.default_quota(&input.service_code, &input.quota_code)
        else {
            return Err(GetServiceQuotaErrorModeled::NoSuchResourceException(
                NoSuchResourceException {
                    message: Some(format!(
                        "no quota with ServiceCode '{}' and QuotaCode '{}' exists",
                        input.service_code, input.quota_code
                    )),
                },
            )
            .into());
        };
        let resource_adjustable = default.resource_adjustable();

        if let Some(context_id) = input.context_id.as_deref() {
            if !resource_adjustable {
                return Err(GetServiceQuotaErrorModeled::IllegalArgumentException(
                    IllegalArgumentException {
                        message: Some(format!(
                            "ContextId '{context_id}' was provided for QuotaCode '{}', which is \
                             adjustable at the ACCOUNT level only and has no quota context",
                            input.quota_code
                        )),
                    },
                )
                .into());
            }
        }

        let quota = quotas_state.applied_quota_view(
            &region,
            &input.service_code,
            &input.quota_code,
            input.context_id.as_deref(),
        );

        Ok(GetServiceQuotaOutput { quota })
    }
}
