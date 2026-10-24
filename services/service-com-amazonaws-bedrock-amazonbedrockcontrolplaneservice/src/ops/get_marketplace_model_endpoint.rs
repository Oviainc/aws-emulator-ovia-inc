// SPDX-License-Identifier: CC0-1.0

//! @op GetMarketplaceModelEndpoint
//!
//! Reports the Bedrock Marketplace model endpoint an `endpointArn` names out of
//! [`crate::state`], or refuses an ARN that names no stored endpoint with 400
//! `ValidationException`.
//!
//! THE REFUSAL CODE IS PER-OPERATION, and recorded: for the same ARN that names no
//! endpoint, the corpus `simparity-behavioral-ga-prod-v1` records Amazon Bedrock answering
//! this operation with 400 `ValidationException` while
//! `UpdateMarketplaceModelEndpoint` answers 404 `ResourceNotFoundException`
//! ([`crate::state::MARKETPLACE_ENDPOINT_NOT_FOUND_MESSAGE`]). Both errors are modelled for
//! this operation, so the one the corpus records for it is the one it answers, and its
//! message names the endpoint the ARN names
//! ([`crate::state::marketplace_endpoint_not_found_for_read`]).
//!
//! `@readonly`, and the one write it makes is the status transition the CLOCK already made
//! due ([`crate::state::MarketplaceModelEndpointRecord::settle`]): an endpoint whose
//! `endpointStatus` was owed [`crate::state::ENDPOINT_STATUS_IN_SERVICE`] before this call
//! is reported, and left, serving — which it would have been at that same instant whether
//! or not anybody read it. Nothing else about the endpoint changes.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetMarketplaceModelEndpoint`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Retrieves details about a specific endpoint for a model from Amazon Bedrock Marketplace.
//!
//! ## Input
//!
//! * `endpointArn` (Arn, len ..2048) **required** — The Amazon Resource Name (ARN) of the
//!   endpoint you want to get information about.
//!
//! ## Output
//!
//! * `marketplaceModelEndpoint` (MarketplaceModelEndpoint) — Details about the requested
//!   endpoint.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetMarketplaceModelEndpointError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetMarketplaceModelEndpointError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#GetMarketplaceModelEndpoint")]
#[derive(Debug)]
pub enum GetMarketplaceModelEndpointErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_marketplace_model_endpoint::GetMarketplaceModelEndpointError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetMarketplaceModelEndpointError = crate::scaffolded::Scaffolded<GetMarketplaceModelEndpointErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetMarketplaceModelEndpointErrorModeled> for GetMarketplaceModelEndpointError {
    fn from(modeled: GetMarketplaceModelEndpointErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetMarketplaceModelEndpoint` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetMarketplaceModelEndpoint<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetMarketplaceModelEndpoint<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetMarketplaceModelEndpoint<Eng>
{
    type Input = GetMarketplaceModelEndpointInput;
    type Output = GetMarketplaceModelEndpointOutput;
    type Error = GetMarketplaceModelEndpointError;

    const NAME: &'static str = "GetMarketplaceModelEndpoint";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_MARKETPLACE_MODEL_ENDPOINT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_MARKETPLACE_MODEL_ENDPOINT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetMarketplaceModelEndpointErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "marketplace model endpoint state is not available on this engine"
                            .to_string(),
                    ),
                },
            )
            .into());
        };
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        let Some(endpoint) = store.marketplace_model_endpoint(&input.endpoint_arn, now) else {
            return Err(
                GetMarketplaceModelEndpointErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(crate::state::marketplace_endpoint_not_found_for_read(
                            &input.endpoint_arn,
                        )),
                    },
                )
                .into(),
            );
        };

        Ok(GetMarketplaceModelEndpointOutput {
            marketplace_model_endpoint: Some(crate::state::marketplace_model_endpoint_view(
                endpoint,
            )),
        })
    }
}
