// SPDX-License-Identifier: CC0-1.0

//! @op ListServiceQuotas
//!
//! Lists the quotas APPLIED to one service: the AWS default of each quota with any
//! value an approved increase has put in force laid over it, read from
//! [`crate::state`] and paginated.
//!
//! * an unknown `ServiceCode`, or a `QuotaCode` the service has no quota for, is
//!   `NoSuchResourceException`;
//! * `QuotaAppliedAtLevel` picks the plane the listing reports, by the crate's one
//!   `AppliedLevelEnum` filter rule ([`crate::state::requested_at_level`]): `ACCOUNT`
//!   (the default when the caller names none) lists one account-wide record per quota,
//!   `RESOURCE` lists one record per resource context a value has been applied to,
//!   and `ALL` lists both;
//! * `MaxResults` / `NextToken` page the listing, and a token this service did not
//!   issue is `InvalidPaginationTokenException`. `MaxResults` bounds what a CALLER may
//!   ask for, not what this listing serves when none is named: a caller that names no
//!   `MaxResults` is served the service's WHOLE applied-quota list in one page with no
//!   `NextToken` ([`crate::state::UNASKED_PAGE_SIZE`]). The quotas come back in the
//!   order the catalog publishes them.
//!
//! Before answering it materialises the increase decisions the clock has already made
//! due, so the value listed here is the value in force at the instant of the call and
//! does not change on a second read.
//!
//! The listing is the service's WHOLE applied-quota list (every quota the catalog
//! seeded in [`crate::state`] publishes for the service, with any approved increase
//! laid over it), which for a service such as EC2 runs past the largest page the
//! model allows, so a walk with a caller-named page size takes many
//! `NextToken`-chained pages.
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
            quotas_state.settle_due_requests(::std::time::SystemTime::now());

            if !quotas_state.knows_service(&input.service_code) {
                return Err(ListServiceQuotasErrorModeled::NoSuchResourceException(
                    NoSuchResourceException {
                        message: Some(crate::state::no_such_service_message(&input.service_code)),
                    },
                )
                .into());
            }

            let mut codes = quotas_state.quota_codes(&input.service_code);
            if let Some(requested) = input.quota_code.as_deref() {
                codes.retain(|code| code == requested);
                if codes.is_empty() {
                    // A `QuotaCode` filter naming a pair the catalog does not hold is
                    // the same fact every other read and write of one quota reports, so
                    // it is stated in the crate's ONE wording for it
                    // ([`crate::state::no_such_quota_message`]) rather than a second
                    // sentence of this handler's own.
                    return Err(ListServiceQuotasErrorModeled::NoSuchResourceException(
                        NoSuchResourceException {
                            message: Some(crate::state::no_such_quota_message(
                                &input.service_code,
                                requested,
                            )),
                        },
                    )
                    .into());
                }
            }

            let level = input.quota_applied_at_level.as_deref();
            let mut quotas: Vec<(String, ServiceQuota)> = Vec::new();
            // Which plane's records the caller asked for is the crate's ONE
            // `AppliedLevelEnum` filter rule ([`crate::state::requested_at_level`]),
            // asked here of the level each plane's records sit at rather than written
            // out again: `ALL` keeps both planes, and a caller that names no level is
            // served the ACCOUNT one. Which quotas HAVE the plane asked for is the
            // view's own rule ([`crate::state::DefaultQuota::has_plane`]): a quota
            // adjustable for an individual resource only has no account-wide record,
            // so the listing carries none for it.
            //
            // Each record is paired with the order key naming its position in this
            // listing: the plane it answers for (the ACCOUNT records come first, and
            // the two level names order the same way), then the quota's PUBLISHED
            // position in the catalog — which is seeded once and never mutated, so it
            // names the same quota on every call — then the resource context, for a
            // record that answers for one. A resource-plane record appearing or going
            // away while a caller walks the listing therefore moves no other record's
            // key, which is what the page token carries.
            if crate::state::requested_at_level(crate::state::LEVEL_ACCOUNT, level) {
                quotas.extend(codes.iter().enumerate().filter_map(|(rank, code)| {
                    quotas_state
                        .applied_quota_view(&region, &input.service_code, code, None)
                        .map(|quota| {
                            let key = crate::state::order_key(&[
                                crate::state::LEVEL_ACCOUNT,
                                &crate::state::rank_key(rank),
                            ]);
                            (key, quota)
                        })
                }));
            }
            if crate::state::requested_at_level(crate::state::LEVEL_RESOURCE, level) {
                for (rank, code) in codes.iter().enumerate() {
                    for context in
                        quotas_state.applied_contexts(&region, &input.service_code, code)
                    {
                        let key = crate::state::order_key(&[
                            crate::state::LEVEL_RESOURCE,
                            &crate::state::rank_key(rank),
                            &context,
                        ]);
                        quotas.extend(
                            quotas_state
                                .applied_quota_view(
                                    &region,
                                    &input.service_code,
                                    code,
                                    Some(&context),
                                )
                                .map(|quota| (key, quota)),
                        );
                    }
                }
            }
            quotas
        };

        // The token names a position in the listing THIS request selected — the
        // region's records for one service, narrowed by the `QuotaCode` and the
        // plane the caller asked for — so a token cut from another selection cannot
        // resume here. A caller that names no plane is served the ACCOUNT one, so
        // the two label the same listing. The position is the order key of the record
        // the page stopped after, so a resource-plane record applied or removed while
        // a caller walks the listing repeats no record and hides none.
        let listing = crate::state::listing_label(
            Self::NAME,
            &[
                Some(&region),
                Some(&input.service_code),
                input.quota_code.as_deref(),
                Some(crate::state::level_filter(
                    input.quota_applied_at_level.as_deref(),
                )),
            ],
        );
        let page = crate::state::paginate_after(
            &listing,
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
