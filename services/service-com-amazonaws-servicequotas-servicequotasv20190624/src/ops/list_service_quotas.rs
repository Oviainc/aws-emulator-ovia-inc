// SPDX-License-Identifier: CC0-1.0

//! @op ListServiceQuotas
//!
//! Lists the quotas APPLIED to one service: the AWS default of each quota with any
//! value an approved increase has put in force laid over it, read from
//! [`crate::state`] and paginated.
//!
//! * an unknown `ServiceCode`, or a `QuotaCode` the service has no quota for, is
//!   `NoSuchResourceException`;
//! * `QuotaAppliedAtLevel` keeps the quotas applied at that level — `ALL` keeps every
//!   one, since a quota applied at any level is applied at all of them;
//! * `MaxResults` / `NextToken` page the listing, and a token this service did not
//!   issue is `InvalidPaginationTokenException`.
//!
//! Before answering it materialises the increase decisions the clock has already made
//! due, so the value listed here is the value in force at the instant of the call and
//! does not change on a second read.
//!
//! The listing is the service's WHOLE applied-quota list (every quota the catalog
//! seeded in [`crate::state`] publishes for the service, with any approved increase
//! laid over it), which for a service such as EC2 runs past the largest page the
//! model allows, so a walk takes many `NextToken`-chained pages.
//!
//! ---
//!
//! # Modeled contract for `ListServiceQuotas`
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
//! * `QuotaCode` (len 1..128, pattern ^[a-zA-Z][a-zA-Z0-9-]{1,128}$)
//! * `MaxResults` (range 1..100), `NextToken` (len 0..2048, pattern ^[a-zA-Z0-9/+]*={0,2}$)
//! * `QuotaAppliedAtLevel` (AppliedLevelEnum: ACCOUNT | ALL | RESOURCE)
//!
//! ## Output
//!
//! * `Quotas` (ServiceQuotaListDefinition), `NextToken`
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
#[smithy_macros::operation_error("com.amazonaws.servicequotas#ListServiceQuotas")]
#[derive(Debug)]
pub enum ListServiceQuotasErrorModeled {
    AccessDeniedException(AccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    InvalidPaginationTokenException(InvalidPaginationTokenException),
    NoSuchResourceException(NoSuchResourceException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type ListServiceQuotasError = crate::scaffolded::Scaffolded<ListServiceQuotasErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<ListServiceQuotasErrorModeled> for ListServiceQuotasError {
    fn from(modeled: ListServiceQuotasErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListServiceQuotas` operation.
pub struct ListServiceQuotas<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListServiceQuotas<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListServiceQuotas<Eng>
{
    type Input = ListServiceQuotasInput;
    type Output = ListServiceQuotasOutput;
    type Error = ListServiceQuotasError;

    const NAME: &'static str = "ListServiceQuotas";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_SERVICE_QUOTAS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_SERVICE_QUOTAS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(&ctx.parts().headers);
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListServiceQuotasErrorModeled::ServiceException(ServiceException {
                message: Some("the Service Quotas state store is unavailable".to_string()),
            })
        })?;

        let quotas = {
            let mut quotas_state = state.lock();
            quotas_state.settle_due_requests(&region, ::std::time::SystemTime::now());

            if !quotas_state.knows_service(&input.service_code) {
                return Err(ListServiceQuotasErrorModeled::NoSuchResourceException(
                    NoSuchResourceException {
                        message: Some(format!(
                            "no service quotas are available for ServiceCode '{}'",
                            input.service_code
                        )),
                    },
                )
                .into());
            }

            let mut codes = quotas_state.quota_codes(&input.service_code);
            if let Some(requested) = input.quota_code.as_deref() {
                codes.retain(|code| code == requested);
                if codes.is_empty() {
                    return Err(ListServiceQuotasErrorModeled::NoSuchResourceException(
                        NoSuchResourceException {
                            message: Some(format!(
                                "ServiceCode '{}' has no quota with QuotaCode '{requested}'",
                                input.service_code
                            )),
                        },
                    )
                    .into());
                }
            }

            codes
                .into_iter()
                .filter_map(|code| {
                    quotas_state.applied_quota_view(&region, &input.service_code, &code)
                })
                .filter(|quota| applied_at_level(quota, input.quota_applied_at_level.as_deref()))
                .collect::<Vec<_>>()
        };

        let page = crate::state::paginate(
            Self::NAME,
            quotas,
            input.max_results,
            input.next_token.as_deref(),
        )
        .map_err(|_| {
            ListServiceQuotasErrorModeled::InvalidPaginationTokenException(
                InvalidPaginationTokenException {
                    message: Some(
                        "NextToken does not name a position in this listing".to_string(),
                    ),
                },
            )
        })?;

        Ok(ListServiceQuotasOutput {
            quotas: Some(page.items),
            next_token: page.next_token,
        })
    }
}

/// Whether a quota belongs in a listing filtered to `requested` level.
///
/// `ALL` is the level that covers every other one, so it keeps the whole listing; any
/// other value keeps the quotas applied at exactly that level.
fn applied_at_level(quota: &ServiceQuota, requested: Option<&str>) -> bool {
    match requested {
        None => true,
        Some(level) if level == crate::state::LEVEL_ALL => true,
        Some(level) => quota.quota_applied_at_level.as_deref() == Some(level),
    }
}
