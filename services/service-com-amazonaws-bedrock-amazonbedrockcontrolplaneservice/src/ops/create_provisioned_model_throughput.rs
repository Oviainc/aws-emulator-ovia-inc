// SPDX-License-Identifier: CC0-1.0

//! @op CreateProvisionedModelThroughput
//!
//! Buys a Provisioned Throughput over a base model: it mints the `ProvisionedModelArn`
//! every other Provisioned Throughput operation addresses, writes the name, model units
//! and commitment the request bought into [`crate::state`], and answers that ARN.
//!
//! The rules it enforces, each out of the model or out of what Amazon Bedrock grants
//! rather than out of one request:
//!
//! * model units come out of the quota the account has been GRANTED for them, and which
//!   quota that is depends on whether the request carries a `commitmentDuration`
//!   ([`crate::state::granted_model_units`]): a create asking for more model units than
//!   are granted is refused with 400 `ServiceQuotaExceededException`. That quota is only
//!   reached for a purchase the model can back at all: a request that omits
//!   `commitmentDuration` asks for a no-commitment Provisioned Throughput, which AWS
//!   offers only over the base models it publishes one for
//!   ([`crate::state::supports_no_commitment_provisioned_throughput`]), so over any other
//!   model that purchase is refused with 400 `ValidationException`.
//! * `provisionedModelName` is the OTHER spelling of `ProvisionedModelId`
//!   (`smithy_query(query="operation",operation="GetProvisionedModelThroughput")`), so two
//!   live Provisioned Throughputs of one name in one region could not both be addressed by
//!   it: a second create of a live name is refused with 400 `ValidationException`.
//! * `modelId` names the base model the throughput is bought over, as a model ARN or a bare
//!   model id ([`crate::state::FoundationModelCatalog::resolve_base_model`]). An identifier
//!   that names no model of this service is refused as the spelling it IS
//!   ([`crate::state::unresolved_model`]): 400 `ValidationException` carrying
//!   [`crate::state::INVALID_MODEL_IDENTIFIER_MESSAGE`] for the bare model-id spelling,
//!   which is no ARN to report missing, and 404 `ResourceNotFoundException` for an ARN of
//!   one of this service's own model resources. A model that EXISTS and is offered under no
//!   Provisioned Throughput is refused with 400 `ValidationException`: one the catalog
//!   carries without it ([`crate::state::supports_provisioned_throughput`]), and one this
//!   service recognises and offers no longer
//!   ([`crate::state::RETIRED_FOUNDATION_MODELS`]), which is offered under nothing at all.
//! * `commitmentDuration` names a term, so the stored record carries the instant that term
//!   ends ([`crate::state::commitment_term`]) and a read reports it as
//!   `commitmentExpirationTime`.
//! * `@idempotencyToken`: a replayed `clientRequestToken` answers with the ARN the first
//!   request minted and buys nothing a second time.
//!
//! Amazon Bedrock provisions the model units after this call returns, so the Provisioned
//! Throughput is stored `Creating` and owed
//! [`crate::state::PROVISIONED_MODEL_STATUS_IN_SERVICE`]
//! [`crate::state::PROVISIONED_MODEL_SETTLE_AFTER`] from now.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateProvisionedModelThroughput`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Creates dedicated throughput for a base or custom model with the model units and for the
//! duration that you specify. For pricing details, see Amazon Bedrock Pricing. For more
//! information, see Provisioned Throughput in the Amazon Bedrock User Guide.
//!
//! ## Input
//!
//! * `clientRequestToken` (IdempotencyToken, len 1..256, pattern
//!   ^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$, @idempotencyToken) — A unique,
//!   case-sensitive identifier to ensure that the API request completes no more than one
//!   time. If this token matches a previous request, Amazon Bedrock ignores the request, but
//!   does not return an error. For more information, see Ensuring idempotency in the Amazon
//!   S3 User Guide.
//! * `commitmentDuration` (CommitmentDuration, one of: OneMonth | SixMonths) — The commitment
//!   duration requested for the Provisioned Throughput. Billing occurs hourly and is
//!   discounted for longer commitment terms. To request a no-commit Provisioned Throughput,
//!   omit this field. Custom models support all levels of commitment. To see which base
//!   models support no commitment, see Supported regions and models for Provisioned
//!   Throughput in the Amazon Bedrock User Guide
//! * `modelId` (ModelIdentifier, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:(([0-9]{12}:custom-model/((imported)|([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}))((
//!   …) **required** — The Amazon Resource Name (ARN) or name of the model to associate with
//!   this Provisioned Throughput. For a list of models for which you can purchase Provisioned
//!   Throughput, see Amazon Bedrock model IDs for purchasing Provisioned Throughput in the
//!   Amazon Bedrock User Guide.
//! * `modelUnits` (PositiveInteger, range 1..) **required** — Number of model units to
//!   allocate. A model unit delivers a specific throughput level for the specified model. The
//!   throughput level of a model unit specifies the total number of input and output tokens
//!   that it can process and generate within a span of one minute. By default, your account
//!   has no model units for purchasing Provisioned Throughputs with commitment. You must
//!   first visit the Amazon Web Services support center to request MUs. For model unit
//!   quotas, see Provisioned Throughput quotas in the Amazon Bedrock User Guide. For more
//!   information about what an MU specifies, contact your Amazon Web Services account
//!   manager.
//! * `provisionedModelName` (ProvisionedModelName, len 1..63, pattern ^([0-9a-zA-Z][_-]?)+$)
//!   **required** — The name for this Provisioned Throughput.
//! * `tags` (TagList, len 0..200) — Tags to associate with this Provisioned Throughput.
//!
//! ## Output
//!
//! * `provisionedModelArn` (ProvisionedModelArn, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:provisioned-model/[a-z0-9]{12}$)
//!   **required** — The Amazon Resource Name (ARN) for this Provisioned Throughput.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ServiceQuotaExceededException` (HTTP 400) — The number of requests exceeds the service
//!   quota. Resubmit your request later.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `TooManyTagsException` (HTTP 400) — The request contains more tags than can be
//!   associated with a resource (50 tags per resource). The maximum number of tags includes
//!   both existing tags and those included in your current request.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateProvisionedModelThroughputError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateProvisionedModelThroughputError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#CreateProvisionedModelThroughput")]
#[derive(Debug)]
pub enum CreateProvisionedModelThroughputErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    TooManyTagsException(TooManyTagsException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_provisioned_model_throughput::CreateProvisionedModelThroughputError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateProvisionedModelThroughputError = crate::scaffolded::Scaffolded<CreateProvisionedModelThroughputErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateProvisionedModelThroughputErrorModeled> for CreateProvisionedModelThroughputError {
    fn from(modeled: CreateProvisionedModelThroughputErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateProvisionedModelThroughput` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateProvisionedModelThroughput<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateProvisionedModelThroughput<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateProvisionedModelThroughput<Eng>
{
    type Input = CreateProvisionedModelThroughputInput;
    type Output = CreateProvisionedModelThroughputOutput;
    type Error = CreateProvisionedModelThroughputError;

    const NAME: &'static str = "CreateProvisionedModelThroughput";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_PROVISIONED_MODEL_THROUGHPUT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_PROVISIONED_MODEL_THROUGHPUT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 201u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                CreateProvisionedModelThroughputErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "provisioned throughput state is not available on this engine"
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
                    .provisioned_model_create_tokens
                    .get(&(region.clone(), token.clone()))
            })
            .cloned();
        if let Some(minted) = replayed {
            if store.provisioned_model_throughputs.contains_key(&minted) {
                return Ok(CreateProvisionedModelThroughputOutput {
                    provisioned_model_arn: minted,
                });
            }
        }

        let tags = input.tags.unwrap_or_default();
        if tags.len() > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(
                CreateProvisionedModelThroughputErrorModeled::TooManyTagsException(
                    TooManyTagsException {
                        message: Some(crate::state::too_many_tags_message()),
                        resource_name: None,
                    },
                )
                .into(),
            );
        }

        if store
            .provisioned_model_names
            .contains_key(&(region.clone(), input.provisioned_model_name.clone()))
        {
            return Err(
                CreateProvisionedModelThroughputErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(format!(
                            "provisionedModelName {} is already used by a Provisioned Throughput \
                             in this account in {region}",
                            input.provisioned_model_name
                        )),
                    },
                )
                .into(),
            );
        }

        // What the `modelId` names decides WHICH refusal a model that cannot back this
        // purchase gets: a model the catalog offers without Provisioned Throughput, and a
        // model this service recognises and offers no longer, are both models that exist
        // and are offered under no Provisioned Throughput — one 400 `ValidationException`
        // about the purchase ([`crate::state::MODEL_NOT_SUPPORTED_FOR_PROVISIONED_THROUGHPUT`])
        // — while an identifier naming no model at all is refused as the spelling it is.
        let not_provisionable = || {
            CreateProvisionedModelThroughputErrorModeled::ValidationException(
                ValidationException {
                    message: Some(
                        crate::state::MODEL_NOT_SUPPORTED_FOR_PROVISIONED_THROUGHPUT.to_string(),
                    ),
                },
            )
        };
        let model = match store.foundation_models.resolve_base_model(&input.model_id) {
            crate::state::BaseModelIdentity::Offered(model) => *model,
            crate::state::BaseModelIdentity::Retired => return Err(not_provisionable().into()),
            crate::state::BaseModelIdentity::Unknown => {
                // An identifier that resolves to no model is refused as the spelling it IS
                // — the one reading every model-identifying member shares
                // ([`crate::state::unresolved_model`]): an ARN of one of this service's own
                // model resources is a resource to report missing, while `modelId`'s bare
                // model-id spelling names no ARN at all and is a value this service cannot
                // read as a model, which is an input fault.
                let refusal = crate::state::unresolved_model(
                    &input.model_id,
                    crate::state::BASE_MODEL_RESOURCES,
                );
                let message = Some(refusal.message(&input.model_id));
                return Err(if refusal.missing_resource() {
                    CreateProvisionedModelThroughputErrorModeled::ResourceNotFoundException(
                        ResourceNotFoundException { message },
                    )
                } else {
                    CreateProvisionedModelThroughputErrorModeled::ValidationException(
                        ValidationException { message },
                    )
                }
                .into());
            }
        };
        if !crate::state::supports_provisioned_throughput(&model) {
            return Err(not_provisionable().into());
        }
        // A purchase that omits `commitmentDuration` asks for a no-commitment Provisioned
        // Throughput, which only the base models AWS offers one for can back
        // (`crate::state::supports_no_commitment_provisioned_throughput`): over any other
        // model there is no such purchase to make, so the refusal is the same
        // not-supported one rather than a report about the account's no-commitment quota.
        if input.commitment_duration.is_none()
            && !crate::state::supports_no_commitment_provisioned_throughput(&model)
        {
            return Err(not_provisionable().into());
        }

        // Model units are bought out of the quota the account has been GRANTED for them,
        // and which quota that is depends on whether this create carries a commitment.
        let granted = crate::state::granted_model_units(input.commitment_duration.as_deref());
        if input.model_units > granted {
            let message = if input.commitment_duration.is_none() {
                crate::state::no_commitment_model_units_message(model.model_id, &region)
            } else {
                format!(
                    "Your account is granted {granted} model unit(s) for a Provisioned \
                     Throughput of {} with a commitment, and this request asks for {}.",
                    model.model_id, input.model_units
                )
            };
            return Err(
                CreateProvisionedModelThroughputErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: Some(message),
                    },
                )
                .into(),
            );
        }

        // The account's Provisioned Throughput quota. Enforcing it here is also what
        // bounds this part of the store: without a server-side cap a caller could grow it
        // with nothing but fresh names.
        if store.provisioned_model_throughputs.len()
            >= crate::state::MAX_PROVISIONED_MODEL_THROUGHPUTS_PER_ACCOUNT
        {
            return Err(
                CreateProvisionedModelThroughputErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: Some(format!(
                            "this account already holds the maximum of {} Provisioned \
                             Throughputs; delete one or request a quota increase",
                            crate::state::MAX_PROVISIONED_MODEL_THROUGHPUTS_PER_ACCOUNT
                        )),
                    },
                )
                .into(),
            );
        }

        let id = store.mint_provisioned_model_id(now);
        let arn = crate::state::provisioned_model_arn(&region, &id);
        let model_arn = crate::state::foundation_model_arn(&region, model.model_id);
        let commitment_expiration_time = input
            .commitment_duration
            .as_deref()
            .and_then(crate::state::commitment_term)
            .map(|term| now + term);
        let record = crate::state::ProvisionedModelThroughputRecord {
            arn: arn.clone(),
            name: input.provisioned_model_name.clone(),
            region: region.clone(),
            model_arn: model_arn.clone(),
            // Nothing is in flight on a Provisioned Throughput that was just bought, so
            // the desired association and allocation are the ones it already has.
            desired_model_arn: model_arn.clone(),
            foundation_model_arn: model_arn,
            model_units: input.model_units,
            desired_model_units: input.model_units,
            status: crate::state::PROVISIONED_MODEL_STATUS_CREATING,
            in_service_due_at: Some(now + crate::state::PROVISIONED_MODEL_SETTLE_AFTER),
            commitment_duration: input.commitment_duration,
            commitment_expiration_time,
            tags,
            created_at: now,
            last_modified_at: now,
        };

        if let Some(token) = input.client_request_token {
            store
                .provisioned_model_create_tokens
                .insert((region.clone(), token), arn.clone());
        }
        store
            .provisioned_model_names
            .insert((region, input.provisioned_model_name), arn.clone());
        store
            .provisioned_model_throughputs
            .insert(arn.clone(), record);

        Ok(CreateProvisionedModelThroughputOutput {
            provisioned_model_arn: arn,
        })
    }
}
