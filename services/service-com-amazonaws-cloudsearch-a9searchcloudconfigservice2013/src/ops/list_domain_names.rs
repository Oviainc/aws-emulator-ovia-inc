// SPDX-License-Identifier: CC0-1.0

//! @op ListDomainNames
//!
//! Reports every live search domain of this account as a map of domain name to the
//! CloudSearch API version that domain speaks. A REPORTER: the only state it changes is
//! the transitions the clock already made due.
//!
//! HAND-AUTHORED. The op-stub generator declined to emit a stub for this operation
//! because the model binds it to Smithy's `Unit` input, which it cannot type yet (see
//! `src/ops/SKIPPED_STUBS.md`); its input and output shapes therefore live in
//! `types/list_domain_names_types.rs`, and the rest of this file follows a rendered
//! sibling exactly so the generated `#[smithy_service]` wiring picks it up by name.
//!
//! The map's VALUES are API versions, not statuses: the modelled `DomainNameMap` targets
//! `APIVersion`, documented as "The Amazon CloudSearch API version for a domain:
//! 2011-02-01 or 2013-01-01". A domain created through this model speaks
//! [`crate::state::API_VERSION`].
//!
//! A domain `DeleteDomain` has removed is not reported here even while its record
//! lingers for `DescribeDomains`: it is no longer a domain requests can be sent to.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. Reach
//! the store through [`crate::state::service_state`] and read and write it through the
//! guard it hands back.
//!
//! ---
//!
//! # Modeled contract for `ListDomainNames`
//!
//! From this service's Smithy model
//! (`smithy_query(query="operation",operation="ListDomainNames",model_path="services/service-com-amazonaws-cloudsearch-a9searchcloudconfigservice2013/smithy-model/com-amazonaws-cloudsearch-a9searchcloudconfigservice2013.json")`):
//!
//! Lists all search domains owned by an account.
//!
//! ## Input
//!
//! (no members — `smithy.api#Unit`)
//!
//! ## Output
//!
//! * `DomainNames` (DomainNameMap) — The names of the search domains owned by an
//!   account, mapped to each domain's API version.
//!
//! ## Errors
//!
//! * `BaseException` (HTTP 400) — An error occurred while processing the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudsearch-a9searchcloudconfigservice2013.json");

/// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.cloudsearch#ListDomainNames")]
#[derive(Debug)]
pub enum ListDomainNamesErrorModeled {
    BaseException(BaseException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_domain_names::ListDomainNamesError`).
pub type ListDomainNamesError = crate::scaffolded::Scaffolded<ListDomainNamesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<ListDomainNamesErrorModeled> for ListDomainNamesError {
    fn from(modeled: ListDomainNamesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListDomainNames` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct ListDomainNames<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListDomainNames<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency. Called by the generated `EmulatedService::new`.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListDomainNames<Eng>
{
    type Input = ListDomainNamesInput;
    type Output = ListDomainNamesOutput;
    type Error = ListDomainNamesError;

    const NAME: &'static str = "ListDomainNames";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::LIST_DOMAIN_NAMES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::LIST_DOMAIN_NAMES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input has no members and `ctx` carries request metadata only; the store
        // is reached through `self.engine`.
        let _ = (&input, ctx);

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListDomainNamesErrorModeled::BaseException(BaseException {
                code: None,
                message: Some(
                    "CloudSearch domain state is unavailable on this engine.".to_string(),
                ),
            })
        })?;
        let mut state = state.lock();

        let clock = ::std::time::Instant::now();
        state.advance(clock);

        let domain_names = state
            .live_domains()
            .map(|domain| {
                (
                    domain.domain_name.clone(),
                    crate::state::API_VERSION.to_string(),
                )
            })
            .collect();

        Ok(ListDomainNamesOutput {
            domain_names: Some(domain_names),
        })
    }
}
