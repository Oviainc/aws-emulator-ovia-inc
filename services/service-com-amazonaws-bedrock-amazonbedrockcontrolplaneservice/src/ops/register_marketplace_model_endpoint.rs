// SPDX-License-Identifier: CC0-1.0

//! @op RegisterMarketplaceModelEndpoint
//!
//! Registers the Amazon SageMaker endpoint an `endpointIdentifier` names with Amazon
//! Bedrock Marketplace, recording in [`crate::state`] which Marketplace model the request
//! says is deployed on it, and reports the stored record — so what this call answers is
//! what `GetMarketplaceModelEndpoint` answers for afterwards.
//!
//! * `modelSourceIdentifier` names a HUB CONTENT of the AWS-managed Amazon SageMaker
//!   public hub, and the model it names is resolved FIRST: a name that hub publishes
//!   nothing under ([`crate::state::is_published_marketplace_model`]) is refused 404
//!   `ResourceNotFoundException` naming the content the identifier named
//!   ([`crate::state::marketplace_hub_content_not_found`]) — the recorded refusal, and
//!   recorded ahead of anything about the endpoint, so a request whose endpoint is absent
//!   too is told about the model source it cannot register. The published catalog is a
//!   SUBSET of the hub's contents, so what this account has already DEPLOYED is evidence
//!   of existence too: a registration of the very model source the endpoint the identifier
//!   names already carries is a registration of a model the hub held when
//!   `CreateMarketplaceModelEndpoint` deployed it, and is not refused on the catalog. That
//!   is what keeps the two operations from contradicting each other — nothing this service
//!   let a caller deploy is then unregisterable.
//! * An `endpointIdentifier` this account holds no endpoint under then names nothing to
//!   register, so it is refused 404 `ResourceNotFoundException`
//!   ([`crate::state::MARKETPLACE_ENDPOINT_NOT_FOUND_MESSAGE`]) — the code and the
//!   sentence the sibling writes `UpdateMarketplaceModelEndpoint` and
//!   `DeregisterMarketplaceModelEndpoint` answer for such an ARN. The endpoints Amazon
//!   Bedrock knows are the entries of
//!   [`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::marketplace_model_endpoints`],
//!   whose membership IS the registration, and `Arn` declares no pattern
//!   (`@length(max: 2048)` only), so an identifier that is no ARN at all names no endpoint
//!   either and is refused the same way rather than as a malformed value.
//! * The model source is "the ARN of the model from Amazon Bedrock Marketplace that is
//!   deployed on the endpoint", so the registration stores it against the endpoint and
//!   every later read reports it. A call naming the model source the endpoint already
//!   carries changes nothing, leaving `updatedAt` where it stood.
//! * `status` is [`crate::state::MARKETPLACE_ENDPOINT_REGISTERED`] from this call onwards:
//!   the endpoint is one Bedrock Marketplace knows. The endpoint's own `endpointStatus` is
//!   SageMaker's, which registering with Bedrock does not move, so it keeps the status the
//!   clock has made due for it.
//!
//! SCOPE. Amazon Bedrock registers an endpoint Amazon SageMaker hosts, and this emulator
//! holds no SageMaker endpoint state of its own — the only endpoints it knows are the ones
//! `CreateMarketplaceModelEndpoint` deployed, which is what
//! [`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::marketplace_model_endpoints`]
//! documents. So an endpoint created elsewhere, and an endpoint whose record
//! `DeregisterMarketplaceModelEndpoint` dropped, are both identifiers this service holds
//! nothing under, and this operation refuses them rather than inventing an endpoint it was
//! never told the configuration of. Registering a SageMaker endpoint this service has no
//! record of needs that record to exist first — a SageMaker-side endpoint store this crate
//! can resolve — which is not this operation's to add.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `RegisterMarketplaceModelEndpoint`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Registers an existing Amazon SageMaker endpoint with Amazon Bedrock Marketplace,
//! allowing it to be used with Amazon Bedrock APIs.
//!
//! ## Input
//!
//! * `endpointIdentifier` (Arn, len ..2048) **required** — The ARN of the Amazon SageMaker
//!   endpoint you want to register with Amazon Bedrock Marketplace.
//! * `modelSourceIdentifier` (ModelSourceIdentifier, len ..2048, pattern
//!   arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.*) **required** — The ARN of
//!   the model from Amazon Bedrock Marketplace that is deployed on the endpoint.
//!
//! ## Output
//!
//! * `marketplaceModelEndpoint` (MarketplaceModelEndpoint) **required** — Details about the
//!   registered endpoint.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ServiceUnavailableException` (HTTP 503) — Returned if the service cannot complete the
//!   request.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `RegisterMarketplaceModelEndpointError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `RegisterMarketplaceModelEndpointError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#RegisterMarketplaceModelEndpoint")]
#[derive(Debug)]
pub enum RegisterMarketplaceModelEndpointErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::register_marketplace_model_endpoint::RegisterMarketplaceModelEndpointError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type RegisterMarketplaceModelEndpointError = crate::scaffolded::Scaffolded<RegisterMarketplaceModelEndpointErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<RegisterMarketplaceModelEndpointErrorModeled> for RegisterMarketplaceModelEndpointError {
    fn from(modeled: RegisterMarketplaceModelEndpointErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `RegisterMarketplaceModelEndpoint` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct RegisterMarketplaceModelEndpoint<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> RegisterMarketplaceModelEndpoint<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for RegisterMarketplaceModelEndpoint<Eng>
{
    type Input = RegisterMarketplaceModelEndpointInput;
    type Output = RegisterMarketplaceModelEndpointOutput;
    type Error = RegisterMarketplaceModelEndpointError;

    const NAME: &'static str = "RegisterMarketplaceModelEndpoint";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REGISTER_MARKETPLACE_MODEL_ENDPOINT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REGISTER_MARKETPLACE_MODEL_ENDPOINT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                RegisterMarketplaceModelEndpointErrorModeled::InternalServerException(
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

        // What this account already has deployed on the endpoint the identifier names, when
        // it holds one: the published catalog is a subset of the public hub's contents, so a
        // model source an endpoint of this account was deployed from is one the hub held,
        // and re-registering exactly that is not refused on the catalog.
        let already_deployed = store
            .marketplace_model_endpoints
            .get(&input.endpoint_identifier)
            .is_some_and(|endpoint| {
                endpoint.model_source_identifier == input.model_source_identifier
            });

        // The Marketplace model comes first: the identifier names a hub content of the
        // AWS-managed SageMaker public hub, and a name neither that hub nor this account's
        // own endpoints know is what the request is refused on, whatever the endpoint half
        // of it names.
        let hub_content =
            crate::state::marketplace_model_source_name_of(&input.model_source_identifier);
        if !already_deployed && !crate::state::is_published_marketplace_model(hub_content) {
            return Err(
                RegisterMarketplaceModelEndpointErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(crate::state::marketplace_hub_content_not_found(
                            hub_content,
                        )),
                    },
                )
                .into(),
            );
        }

        // The endpoint is addressed by its ARN, which carries its region, so the request's
        // own signing region decides nothing here. An identifier this account holds no
        // endpoint under names nothing to register, and is refused the way the sibling
        // writes `UpdateMarketplaceModelEndpoint` and `DeregisterMarketplaceModelEndpoint`
        // refuse such an ARN.
        let Some(endpoint) = store.marketplace_model_endpoint(&input.endpoint_identifier, now)
        else {
            return Err(
                RegisterMarketplaceModelEndpointErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(
                            crate::state::MARKETPLACE_ENDPOINT_NOT_FOUND_MESSAGE.to_string(),
                        ),
                    },
                )
                .into(),
            );
        };

        // The registration records which Marketplace model is deployed on the endpoint, so
        // a call naming another model source replaces the stored one and moves the
        // endpoint's `updatedAt`, while one naming the model source it already carries
        // changes nothing.
        if endpoint.model_source_identifier != input.model_source_identifier
            || endpoint.status != crate::state::MARKETPLACE_ENDPOINT_REGISTERED
        {
            endpoint.model_source_identifier = input.model_source_identifier;
            endpoint.status = crate::state::MARKETPLACE_ENDPOINT_REGISTERED;
            endpoint.updated_at = now;
        }

        // Report the STORED record, so the caller is told exactly what a later
        // GetMarketplaceModelEndpoint will find.
        Ok(RegisterMarketplaceModelEndpointOutput {
            marketplace_model_endpoint: crate::state::marketplace_model_endpoint_view(endpoint),
        })
    }
}
