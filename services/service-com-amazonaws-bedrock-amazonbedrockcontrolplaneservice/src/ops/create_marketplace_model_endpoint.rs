// SPDX-License-Identifier: CC0-1.0

//! @op CreateMarketplaceModelEndpoint
//!
//! Mints a Bedrock Marketplace model endpoint and PERSISTS it in [`crate::state`]: the
//! `endpoint/<endpointName>` SageMaker ARN for the signing region
//! ([`crate::state::marketplace_model_endpoint_arn`]), the request's model source,
//! endpoint configuration, EULA acceptance and tags, and the creation instant. The record
//! the caller is told is read back out of the store, so it is what
//! `GetMarketplaceModelEndpoint` answers for.
//!
//! * `endpointName` "must be unique within your Amazon Web Services account and region",
//!   so a name another live endpoint holds in the same region is refused with
//!   `ConflictException`.
//! * The endpoint is reported `endpointStatus`
//!   [`crate::state::ENDPOINT_STATUS_CREATING`] and is owed
//!   [`crate::state::ENDPOINT_STATUS_IN_SERVICE`]
//!   [`crate::state::ENDPOINT_SETTLE_AFTER`] later: the endpoint is hosted by Amazon
//!   SageMaker, which brings it up after the call returns. `status` — the Marketplace
//!   registration — is [`crate::state::MARKETPLACE_ENDPOINT_REGISTERED`] from this call
//!   onwards, because an endpoint Bedrock itself created from a Marketplace model is
//!   registered with Bedrock.
//! * A replay of the same `clientRequestToken` answers with the endpoint that token
//!   already minted (`@idempotencyToken`).
//! * The account's endpoint quota ([`crate::state::MAX_MARKETPLACE_MODEL_ENDPOINTS_PER_ACCOUNT`])
//!   is refused with `ServiceQuotaExceededException`.
//!
//! `tags` is stored as the request carried it: this operation models no
//! `TooManyTagsException`, and `TagList` is `@length(0..200)`, which the boundary already
//! enforces.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateMarketplaceModelEndpoint`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Creates an endpoint for a model from Amazon Bedrock Marketplace. The endpoint is hosted
//! by Amazon SageMaker.
//!
//! ## Input
//!
//! * `acceptEula` (AcceptEula, default false) **required** — Indicates whether you accept the
//!   end-user license agreement (EULA) for the model. Set to true to accept the EULA.
//! * `clientRequestToken` (IdempotencyToken, len 1..256, pattern
//!   ^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$, @idempotencyToken) — A unique,
//!   case-sensitive identifier that you provide to ensure the idempotency of the request.
//!   This token is listed as not required because Amazon Web Services SDKs automatically
//!   generate it for you and set this parameter. If you're not using the Amazon Web Services
//!   SDK or the CLI, you must provide this token or the action will fail.
//! * `endpointConfig` (EndpointConfig) **required** — The configuration for the endpoint,
//!   including the number and type of instances to use.
//! * `endpointName` (EndpointName, len 1..30) **required** — The name of the endpoint. This
//!   name must be unique within your Amazon Web Services account and region.
//! * `modelSourceIdentifier` (ModelSourceIdentifier, len ..2048, pattern
//!   arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.*) **required** — The ARN of
//!   the model from Amazon Bedrock Marketplace that you want to deploy to the endpoint.
//! * `tags` (TagList, len 0..200) — An array of key-value pairs to apply to the underlying
//!   Amazon SageMaker endpoint. You can use these tags to organize and identify your Amazon
//!   Web Services resources.
//!
//! ## Output
//!
//! * `marketplaceModelEndpoint` (MarketplaceModelEndpoint) **required** — Details about the
//!   created endpoint.
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
// `CreateMarketplaceModelEndpointError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateMarketplaceModelEndpointError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#CreateMarketplaceModelEndpoint")]
#[derive(Debug)]
pub enum CreateMarketplaceModelEndpointErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_marketplace_model_endpoint::CreateMarketplaceModelEndpointError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateMarketplaceModelEndpointError = crate::scaffolded::Scaffolded<CreateMarketplaceModelEndpointErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateMarketplaceModelEndpointErrorModeled> for CreateMarketplaceModelEndpointError {
    fn from(modeled: CreateMarketplaceModelEndpointErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateMarketplaceModelEndpoint` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateMarketplaceModelEndpoint<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateMarketplaceModelEndpoint<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateMarketplaceModelEndpoint<Eng>
{
    type Input = CreateMarketplaceModelEndpointInput;
    type Output = CreateMarketplaceModelEndpointOutput;
    type Error = CreateMarketplaceModelEndpointError;

    const NAME: &'static str = "CreateMarketplaceModelEndpoint";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_MARKETPLACE_MODEL_ENDPOINT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_MARKETPLACE_MODEL_ENDPOINT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                CreateMarketplaceModelEndpointErrorModeled::InternalServerException(
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
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        // `@idempotencyToken`: a replay answers with what the first request minted.
        let replayed = input
            .client_request_token
            .as_ref()
            .and_then(|token| {
                store
                    .marketplace_endpoint_create_tokens
                    .get(&(region.clone(), token.clone()))
            })
            .cloned();
        if let Some(minted) = replayed {
            if let Some(existing) = store.marketplace_model_endpoint(&minted, now) {
                return Ok(CreateMarketplaceModelEndpointOutput {
                    marketplace_model_endpoint: crate::state::marketplace_model_endpoint_view(
                        existing,
                    ),
                });
            }
        }

        // The ARN carries the name and the region, so it is also the uniqueness the member
        // documents: "this name must be unique within your Amazon Web Services account and
        // region".
        let arn = crate::state::marketplace_model_endpoint_arn(&region, &input.endpoint_name);
        if store.marketplace_model_endpoints.contains_key(&arn) {
            return Err(
                CreateMarketplaceModelEndpointErrorModeled::ConflictException(ConflictException {
                    message: Some(format!(
                        "a marketplace model endpoint named {} already exists in this account in \
                         {region}",
                        input.endpoint_name
                    )),
                })
                .into(),
            );
        }

        // The account's endpoint quota. Enforcing it here is also what bounds this part of
        // the store: without a server-side cap a caller could grow it with nothing but
        // fresh endpoint names.
        if store.marketplace_model_endpoints.len()
            >= crate::state::MAX_MARKETPLACE_MODEL_ENDPOINTS_PER_ACCOUNT
        {
            return Err(
                CreateMarketplaceModelEndpointErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: Some(format!(
                            "this account already holds the maximum of {} marketplace model \
                             endpoints; delete one or request a quota increase",
                            crate::state::MAX_MARKETPLACE_MODEL_ENDPOINTS_PER_ACCOUNT
                        )),
                    },
                )
                .into(),
            );
        }

        let endpoint = crate::state::MarketplaceModelEndpointRecord {
            arn: arn.clone(),
            name: input.endpoint_name,
            model_source_identifier: input.model_source_identifier,
            endpoint_config: input.endpoint_config,
            accept_eula: input.accept_eula,
            tags: input.tags.unwrap_or_default(),
            status: crate::state::MARKETPLACE_ENDPOINT_REGISTERED,
            // SageMaker brings the endpoint up after this call returns, so it is reported
            // creating now and is owed `InService` once the span has passed.
            endpoint_status: crate::state::ENDPOINT_STATUS_CREATING,
            in_service_due_at: Some(now + crate::state::ENDPOINT_SETTLE_AFTER),
            created_at: now,
            updated_at: now,
            last_update_token: None,
        };

        if let Some(token) = input.client_request_token {
            store
                .marketplace_endpoint_create_tokens
                .insert((region, token), arn.clone());
        }
        store
            .marketplace_model_endpoints
            .insert(arn.clone(), endpoint);

        // Read the response back out of the STORED record, so what the caller is told is
        // exactly what a later GetMarketplaceModelEndpoint will find.
        let stored = store
            .marketplace_model_endpoints
            .get(&arn)
            .expect("the marketplace model endpoint just inserted under this ARN");
        Ok(CreateMarketplaceModelEndpointOutput {
            marketplace_model_endpoint: crate::state::marketplace_model_endpoint_view(stored),
        })
    }
}
