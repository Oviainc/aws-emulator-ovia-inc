// SPDX-License-Identifier: CC0-1.0

//! @op FlushApiCache
//!
//! Flushes the API's cache: every entry the cache holds is discarded, and the cache
//! INSTANCE is left exactly as it was — its settings and the status it is in are
//! properties of the instance, not of the entries it holds. The response carries no
//! members, so what the call leaves behind is the new entry boundary on the stored
//! cache ([`crate::state::ApiCacheRecord::flush`]).
//!
//! A flush addresses the cache through the API that owns it, so an `apiId` that is not
//! an API id is a bad request (400), one that names no API of this account is a
//! not-found (404), and an API that holds no cache has no cache to flush — also a
//! not-found.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `FlushApiCache::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `FlushApiCache`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/smithy-model/com-amazonaws-appsync-awsdeepdishcontrolplaneservice.json")
//! ```
//!
//! Flushes an ApiCache object.
//!
//! ## Input
//!
//! * `apiId` (String) **required** — The API ID.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — The request is not well formed. For example, a value
//!   is invalid or a required field is missing. Check the field values, and then try again.
//! * `ConcurrentModificationException` (HTTP 409) — Another modification is in progress at
//!   this time and it must complete before you can make your change.
//! * `InternalFailureException` (HTTP 500) — An internal AppSync error occurred. Try your
//!   request again.
//! * `NotFoundException` (HTTP 404) — The resource specified in the request was not found.
//!   Check the resource, and then try again.
//! * `UnauthorizedException` (HTTP 401) — You aren't authorized to perform this operation.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `FlushApiCacheError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appsync-awsdeepdishcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `FlushApiCacheError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.appsync#FlushApiCache")]
#[derive(Debug)]
pub enum FlushApiCacheErrorModeled {
    BadRequestException(BadRequestException),
    ConcurrentModificationException(ConcurrentModificationException),
    InternalFailureException(InternalFailureException),
    NotFoundException(NotFoundException),
    UnauthorizedException(UnauthorizedException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::flush_api_cache::FlushApiCacheError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type FlushApiCacheError = crate::scaffolded::Scaffolded<FlushApiCacheErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<FlushApiCacheErrorModeled> for FlushApiCacheError {
    fn from(modeled: FlushApiCacheErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `FlushApiCache` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct FlushApiCache<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> FlushApiCache<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for FlushApiCache<Eng>
{
    type Input = FlushApiCacheInput;
    type Output = FlushApiCacheOutput;
    type Error = FlushApiCacheError;

    const NAME: &'static str = "FlushApiCache";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::FLUSH_API_CACHE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::FLUSH_API_CACHE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Which cache is flushed comes from the deserialized input and the clock; no
        // request metadata takes part, so `ctx` is unused.
        let _ = ctx;

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(FlushApiCacheErrorModeled::InternalFailureException(
                InternalFailureException {
                    message: Some("the appsync store is unavailable on this engine".to_string()),
                },
            )
            .into());
        };

        // An `apiId` that is not an API id at all is a malformed request, not a
        // missing resource (see `crate::state::is_api_id`).
        if !crate::state::is_api_id(&input.api_id) {
            return Err(FlushApiCacheErrorModeled::BadRequestException(BadRequestException {
                message: Some(crate::state::invalid_api_id_message(&input.api_id)),
                ..Default::default()
            })
            .into());
        }

        let now = std::time::SystemTime::now();
        let mut guard = state.lock();
        // Apply what the clock owes before reading, so a provisioning instance's
        // pending transition is not re-decided by this call.
        guard.settle_api_caches(now);

        let Some(api) = guard.graphql_apis.get_mut(&input.api_id) else {
            return Err(FlushApiCacheErrorModeled::NotFoundException(NotFoundException {
                message: Some(crate::state::API_NOT_FOUND_MESSAGE.to_string()),
            })
            .into());
        };
        // An API that holds no cache — because none was created, or because a create
        // was refused — has no cache to flush.
        let Some(cache) = api.api_cache.as_mut() else {
            return Err(FlushApiCacheErrorModeled::NotFoundException(NotFoundException {
                message: Some(crate::state::CACHE_NOT_FOUND_MESSAGE.to_string()),
            })
            .into());
        };

        // Discard the entries, leaving the instance — its settings and the status it is
        // in — exactly as the create or the last update left it, so the next
        // `GetApiCache` reports what it reported before.
        cache.flush(now);

        // The modelled response carries no members.
        Ok(FlushApiCacheOutput {})
    }
}
