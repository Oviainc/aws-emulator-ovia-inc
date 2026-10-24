// SPDX-License-Identifier: CC0-1.0

//! @op GetSignedBluinsightsUrl
//!
//! Reports the single-sign-on AWS Blu Insights URL for the account and region this
//! request was signed for.
//!
//! `@readonly`: this handler only reads the request's own credential scope and
//! computes the URL from it. It never writes, so a read leaves the next caller seeing
//! exactly what it saw, and two reads in one region answer the same URL.
//!
//! The URL is COMPUTED, not stored and not invented per call: it is built on the
//! EMULATOR's own endpoint — the authority the request arrived on
//! ([`crate::state::request_endpoint`]) — and names the account and the region the
//! caller signed for, with a federation token derived from exactly those two values
//! ([`crate::state::signed_bluinsights_url`]). It deliberately does NOT name
//! `bluinsights.aws`, the host the real web application is published under: this
//! emulator runs no Blu Insights application, so a URL pointing there would send a
//! caller out of the emulator. The model constrains `signedBiUrl` only as a `String`,
//! and the recording this was filed against asserts its presence, not its value.
//!
//! Hand-authored rather than scaffolded: the op-stub generator declined this operation
//! because its input is `smithy.api#Unit` (recorded in `src/ops/SKIPPED_STUBS.md`), so
//! it had no handler file and no entry in the service's `operations(...)` allow-list
//! and was answered by the framework's unroutable-operation path — an HTTP 501 that is
//! not one of this operation's modelled outcomes (its errors are
//! AccessDenied/InternalServer/Throttling). `build.rs` derives that allow-list from
//! the `//! @op <Name>` header of every `src/ops/*.rs`, so this file is what wires it.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The
//! store is reached through [`crate::state::service_state`]; this operation reads no
//! record out of it, because m2 persists no Blu Insights resource — the URL is a
//! property of the account and region, which `ctx` carries.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetSignedBluinsightsUrl`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-m2-awssupernovacontrolplaneservice/smithy-model/com-amazonaws-m2-awssupernovacontrolplaneservice.json")
//! ```
//!
//! Gets a single sign-on URL that can be used to connect to AWS Blu Insights.
//!
//! ## Input
//!
//! `smithy.api#Unit` — no members. `GET /signed-bi-url`.
//!
//! ## Output
//!
//! * `signedBiUrl` (String) **required** — Single sign-on AWS Blu Insights URL.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The account or role doesn't have the right
//!   permissions to make the request.
//! * `InternalServerException` (HTTP 500) — An unexpected error occurred during the
//!   processing of the request.
//! * `ThrottlingException` (HTTP 429) — The number of requests made exceeds the limit.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetSignedBluinsightsUrlError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-m2-awssupernovacontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.m2#GetSignedBluinsightsUrl")]
#[derive(Debug)]
pub enum GetSignedBluinsightsUrlErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_signed_bluinsights_url::GetSignedBluinsightsUrlError`).
pub type GetSignedBluinsightsUrlError =
    crate::scaffolded::Scaffolded<GetSignedBluinsightsUrlErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the
/// handler writes `Err(SomeModeledError.into())` rather than naming the wrapper
/// variant.
impl From<GetSignedBluinsightsUrlErrorModeled> for GetSignedBluinsightsUrlError {
    fn from(modeled: GetSignedBluinsightsUrlErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetSignedBluinsightsUrl` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetSignedBluinsightsUrl<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetSignedBluinsightsUrl<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetSignedBluinsightsUrl<Eng>
{
    type Input = GetSignedBluinsightsUrlInput;
    type Output = GetSignedBluinsightsUrlOutput;
    type Error = GetSignedBluinsightsUrlError;

    const NAME: &'static str = "GetSignedBluinsightsUrl";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_SIGNED_BLUINSIGHTS_URL_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_SIGNED_BLUINSIGHTS_URL_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input is `smithy.api#Unit`, so there is nothing to read from it: which
        // URL this reports is decided by the account and the region the request was
        // signed for.
        let _ = &input;

        // The store is reached the way every other handler in this crate reaches it,
        // so an engine this service is not mounted on is reported as the modelled
        // server fault rather than answered with a URL. Nothing is READ out of it:
        // m2 persists no Blu Insights resource, so this is a mount check and no guard
        // is held while the URL is computed.
        if crate::state::service_state(&*self.engine).is_none() {
            return Err(GetSignedBluinsightsUrlErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the m2 application store is unavailable".to_string(),
                    retry_after_seconds: 0,
                },
            )
            .into());
        }

        let region = crate::state::request_region(ctx.parts());
        Ok(GetSignedBluinsightsUrlOutput {
            signed_bi_url: crate::state::signed_bluinsights_url(
                &crate::state::request_endpoint(ctx.parts(), &region),
                ::engine::emulator_account_id(),
                &region,
            ),
        })
    }
}
