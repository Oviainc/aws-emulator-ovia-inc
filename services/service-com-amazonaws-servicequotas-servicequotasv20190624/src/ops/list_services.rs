// SPDX-License-Identifier: CC0-1.0

//! @op ListServices
//!
//! Lists the Amazon Web Services services integrated with Service Quotas, from the
//! seeded catalog in [`crate::state`], one page at a time.
//!
//! The catalog is the whole integrated-service list — the documented way to discover
//! a valid `ServiceCode` — so it runs past the largest page the model allows and a
//! walk with a caller-named page size takes many `NextToken`-chained pages.
//!
//! `MaxResults` is the page size and `NextToken` resumes where the previous page
//! stopped; the last page carries no `NextToken`. A caller that names no
//! `MaxResults` gets the WHOLE catalog in one page and no `NextToken`
//! ([`crate::state::UNASKED_PAGE_SIZE`]): `MaxResults`' 1..100 range
//! bounds what a caller may ask for, not what this listing returns unasked, and the
//! recorded answer to an empty `ListServices` body carries the inventory whole. A
//! token this service did not issue is refused with
//! `InvalidPaginationTokenException`.
//!
//! The catalog is seeded in `ComAmazonawsServicequotasServicequotasv20190624State::default`
//! and never mutated, so this operation only reads.
//!
//! ---
//!
//! # Modeled contract for `ListServices`
//!
//! For the FULL text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! ## Input
//!
//! * `MaxResults` (MaxResults, range 1..100) — the maximum number of results on each
//!   page of the response.
//! * `NextToken` (NextToken, len 0..2048, pattern ^[a-zA-Z0-9/+]*={0,2}$) — where the
//!   output should continue from.
//!
//! ## Output
//!
//! * `NextToken` — present when more output is available.
//! * `Services` (ServiceInfoListDefinition) — the service names and service codes.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403), `IllegalArgumentException` (HTTP 400),
//!   `InvalidPaginationTokenException` (HTTP 400), `ServiceException` (HTTP 500),
//!   `TooManyRequestsException` (HTTP 429).

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json");

/// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.servicequotas#ListServices")]
#[derive(Debug)]
pub enum ListServicesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    InvalidPaginationTokenException(InvalidPaginationTokenException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type ListServicesError = crate::scaffolded::Scaffolded<ListServicesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<ListServicesErrorModeled> for ListServicesError {
    fn from(modeled: ListServicesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListServices` operation.
pub struct ListServices<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListServices<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListServices<Eng>
{
    type Input = ListServicesInput;
    type Output = ListServicesOutput;
    type Error = ListServicesError;

    const NAME: &'static str = "ListServices";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_SERVICES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_SERVICES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The catalog is the same in every region, so this operation reads no
        // region from the request.
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListServicesErrorModeled::ServiceException(ServiceException {
                message: Some("the Service Quotas state store is unavailable".to_string()),
            })
        })?;

        let services = state.lock().services();
        // The unasked page is the WHOLE inventory: `MaxResults` bounds only what a
        // caller asks for.
        //
        // This listing has no input that selects WHICH records it holds (and the
        // catalog is the same in every region), so there is one listing here and its
        // label is the operation's name: every caller's token names a position in it.
        let listing = crate::state::listing_label(Self::NAME, &[]);
        let page = crate::state::paginate(
            &listing,
            services,
            input.max_results,
            input.next_token.as_deref(),
        )
        .map_err(|_| {
            ListServicesErrorModeled::InvalidPaginationTokenException(
                InvalidPaginationTokenException {
                    message: Some(
                        crate::service::refusals::INVALID_PAGINATION_TOKEN_MESSAGE.to_string(),
                    ),
                },
            )
        })?;

        Ok(ListServicesOutput {
            services: Some(page.items),
            next_token: page.next_token,
        })
    }
}
