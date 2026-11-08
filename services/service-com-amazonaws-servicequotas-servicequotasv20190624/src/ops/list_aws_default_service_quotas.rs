// SPDX-License-Identifier: CC0-1.0

//! @op ListAWSDefaultServiceQuotas
//!
//! Lists the AWS DEFAULT quotas of one service, from the seeded default map in
//! [`crate::state`], paginated. Every entry is the record
//! `GetAWSDefaultServiceQuota` returns for that pair, built by the same view, so the
//! two agree field for field — and no entry is affected by an applied increase.
//!
//! `ServiceCode` resolves against the ONE service catalog `ListServices` enumerates
//! (the seeded catalog in [`crate::state`]): every code that catalog holds answers
//! HTTP 200 with a `Quotas` list — empty for a service whose published defaults are
//! not seeded, since the code itself exists — and `NoSuchResourceException` is
//! reserved for a code the catalog does not know (one that is not an AWS service
//! code). A `NextToken` this service did not issue for THIS listing is
//! `InvalidPaginationTokenException` — a token names a position in the set the
//! request selected, so one cut from another `ServiceCode`'s listing resumes nothing.
//!
//! `MaxResults` bounds what a CALLER may ask for, not what this listing serves when
//! none is named: a caller that names no `MaxResults` is served the service's WHOLE
//! default-quota set in one page with no `NextToken`
//! ([`crate::state::UNASKED_PAGE_SIZE`]).
//!
//! The quotas come back in the order the catalog publishes them, and each entry's
//! `QuotaArn` is the AWS-DEFAULT form, whose account segment is empty: a default
//! quota is AWS's published value and belongs to no account.
//!
//! The listing is the service's WHOLE published default-quota list (the catalog
//! seeded in [`crate::state`]), which for a service such as EC2 runs well past the
//! largest page the model allows, so a walk with a caller-named page size takes many
//! `NextToken`-chained pages.
//!
//! ---
//!
//! # Modeled contract for `ListAWSDefaultServiceQuotas`
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
//! * `MaxResults` (range 1..100), `NextToken` (len 0..2048, pattern ^[a-zA-Z0-9/+]*={0,2}$)
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
#[smithy_macros::operation_error("com.amazonaws.servicequotas#ListAWSDefaultServiceQuotas")]
#[derive(Debug)]
pub enum ListAWSDefaultServiceQuotasErrorModeled {
    AccessDeniedException(AccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    InvalidPaginationTokenException(InvalidPaginationTokenException),
    NoSuchResourceException(NoSuchResourceException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type ListAWSDefaultServiceQuotasError =
    crate::scaffolded::Scaffolded<ListAWSDefaultServiceQuotasErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<ListAWSDefaultServiceQuotasErrorModeled> for ListAWSDefaultServiceQuotasError {
    fn from(modeled: ListAWSDefaultServiceQuotasErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListAWSDefaultServiceQuotas` operation.
pub struct ListAWSDefaultServiceQuotas<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListAWSDefaultServiceQuotas<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListAWSDefaultServiceQuotas<Eng>
{
    type Input = ListAWSDefaultServiceQuotasInput;
    type Output = ListAWSDefaultServiceQuotasOutput;
    type Error = ListAWSDefaultServiceQuotasError;

    const NAME: &'static str = "ListAWSDefaultServiceQuotas";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_AWS_DEFAULT_SERVICE_QUOTAS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_AWS_DEFAULT_SERVICE_QUOTAS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(&ctx.parts().headers);
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListAWSDefaultServiceQuotasErrorModeled::ServiceException(ServiceException {
                message: Some("the Service Quotas state store is unavailable".to_string()),
            })
        })?;

        let quotas = {
            let quotas_state = state.lock();
            if !quotas_state.knows_service(&input.service_code) {
                return Err(
                    ListAWSDefaultServiceQuotasErrorModeled::NoSuchResourceException(
                        NoSuchResourceException {
                            message: Some(
                                crate::service::refusals::SERVICE_NOT_AVAILABLE_MESSAGE
                                    .to_string(),
                            ),
                        },
                    )
                    .into(),
                );
            }
            quotas_state
                .quota_codes(&input.service_code)
                .into_iter()
                .filter_map(|code| {
                    quotas_state.default_quota_view(&region, &input.service_code, &code)
                })
                .collect::<Vec<_>>()
        };

        // The token names a position in the listing THIS request selected — one
        // service's defaults, as this region publishes them — so a token cut from
        // another selection cannot resume here.
        let listing = crate::state::listing_label(
            Self::NAME,
            &[Some(&region), Some(&input.service_code)],
        );
        let page = crate::state::paginate(
            &listing,
            quotas,
            input.max_results,
            input.next_token.as_deref(),
        )
        .map_err(|_| {
            ListAWSDefaultServiceQuotasErrorModeled::InvalidPaginationTokenException(
                InvalidPaginationTokenException {
                    message: Some(
                        crate::service::refusals::INVALID_PAGINATION_TOKEN_MESSAGE.to_string(),
                    ),
                },
            )
        })?;

        Ok(ListAWSDefaultServiceQuotasOutput {
            quotas: Some(page.items),
            next_token: page.next_token,
        })
    }
}
