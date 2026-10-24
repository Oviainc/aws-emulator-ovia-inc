// SPDX-License-Identifier: CC0-1.0

//! @op UpdateMarketplaceModelEndpoint
//!
//! Replaces a stored endpoint's `endpointConfig` in [`crate::state`] and reports the
//! endpoint as stored, so what this answers is what `GetMarketplaceModelEndpoint` finds
//! next.
//!
//! * An `endpointArn` no stored endpoint carries is refused with 404
//!   `ResourceNotFoundException`, worded
//!   [`crate::state::MARKETPLACE_ENDPOINT_NOT_FOUND_MESSAGE`] — nothing is created by an
//!   update.
//! * The configuration is REPLACED whole rather than merged: `endpointConfig` is a
//!   `@required` union, so the request carries the whole new configuration.
//! * The endpoint's `endpointStatus` moves to
//!   [`crate::state::ENDPOINT_STATUS_UPDATING`] and is owed
//!   [`crate::state::ENDPOINT_STATUS_IN_SERVICE`]
//!   [`crate::state::ENDPOINT_SETTLE_AFTER`] later, which the next call that touches the
//!   endpoint materialises; `status` (the Marketplace registration) is unchanged.
//! * `updatedAt` moves to the instant of this call; `createdAt` never moves.
//! * A RETRY carrying the `clientRequestToken` this endpoint's last update carried
//!   (`@idempotencyToken`) applies nothing a second time and answers with the endpoint as
//!   it stands.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateMarketplaceModelEndpoint`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Updates the configuration of an existing endpoint for a model from Amazon Bedrock
//! Marketplace.
//!
//! ## Input
//!
//! * `clientRequestToken` (IdempotencyToken, len 1..256, pattern
//!   ^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$, @idempotencyToken) — A unique,
//!   case-sensitive identifier that you provide to ensure the idempotency of the request.
//!   This token is listed as not required because Amazon Web Services SDKs automatically
//!   generate it for you and set this parameter. If you're not using the Amazon Web Services
//!   SDK or the CLI, you must provide this token or the action will fail.
//! * `endpointArn` (Arn, len ..2048) **required** — The Amazon Resource Name (ARN) of the
//!   endpoint you want to update.
//! * `endpointConfig` (EndpointConfig) **required** — The new configuration for the endpoint,
//!   including the number and type of instances to use.
//!
//! ## Output
//!
//! * `marketplaceModelEndpoint` (MarketplaceModelEndpoint) **required** — Details about the
//!   updated endpoint.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `ConflictException` (HTTP 400) — Error occurred because of a conflict while performing
//!   an operation.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ServiceQuotaExceededException` (HTTP 400) — The number of requests exceeds the service
//!   quota. Resubmit your request later.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateMarketplaceModelEndpointError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateMarketplaceModelEndpointError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#UpdateMarketplaceModelEndpoint")]
#[derive(Debug)]
pub enum UpdateMarketplaceModelEndpointErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_marketplace_model_endpoint::UpdateMarketplaceModelEndpointError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateMarketplaceModelEndpointError = crate::scaffolded::Scaffolded<UpdateMarketplaceModelEndpointErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateMarketplaceModelEndpointErrorModeled> for UpdateMarketplaceModelEndpointError {
    fn from(modeled: UpdateMarketplaceModelEndpointErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateMarketplaceModelEndpoint` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateMarketplaceModelEndpoint<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateMarketplaceModelEndpoint<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateMarketplaceModelEndpoint<Eng>
{
    type Input = UpdateMarketplaceModelEndpointInput;
    type Output = UpdateMarketplaceModelEndpointOutput;
    type Error = UpdateMarketplaceModelEndpointError;

    const NAME: &'static str = "UpdateMarketplaceModelEndpoint";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_MARKETPLACE_MODEL_ENDPOINT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_MARKETPLACE_MODEL_ENDPOINT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                UpdateMarketplaceModelEndpointErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "marketplace model endpoint state is not available on this engine"
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        };
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        // The endpoint is addressed by its ARN, which carries its region, so the request's
        // own signing region decides nothing here.
        let Some(endpoint) = store.marketplace_model_endpoint(&input.endpoint_arn, now) else {
            return Err(
                UpdateMarketplaceModelEndpointErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(
                            crate::state::MARKETPLACE_ENDPOINT_NOT_FOUND_MESSAGE.to_string(),
                        ),
                    },
                )
                .into(),
            );
        };

        // `@idempotencyToken`: a retry of the update this endpoint already applied changes
        // nothing and answers with the endpoint as it stands.
        let is_retry = input
            .client_request_token
            .as_ref()
            .is_some_and(|token| endpoint.last_update_token.as_deref() == Some(token.as_str()));
        if !is_retry {
            endpoint.endpoint_config = input.endpoint_config;
            endpoint.updated_at = now;
            endpoint.last_update_token = input.client_request_token;
            endpoint.begin(crate::state::ENDPOINT_STATUS_UPDATING, now);
        }

        // Report the STORED record, so the caller is told exactly what a later
        // GetMarketplaceModelEndpoint will find.
        Ok(UpdateMarketplaceModelEndpointOutput {
            marketplace_model_endpoint: crate::state::marketplace_model_endpoint_view(endpoint),
        })
    }
}
