// SPDX-License-Identifier: CC0-1.0

//! @op CreateInferenceProfile
//!
//! Mints an APPLICATION inference profile and PERSISTS it in [`crate::state`]: the
//! `application-inference-profile/<id>` ARN for the signing region, the request's name,
//! description and tags, the foundation models the request's `modelSource` names, the
//! `ACTIVE` status and the creation instant. The ARN and the status the caller is told are
//! read back out of the stored record, so they are what `GetInferenceProfile` answers for.
//!
//! The profile's `models` is derived from `modelSource.copyFrom`, the only member that
//! union carries: the source is the ARN of a foundation model or of a system-defined
//! inference profile, and either way the profile tracks the foundation model it names
//! ([`tracked_models`]).
//!
//! A replay of the same `clientRequestToken` answers with the profile that token already
//! minted (`@idempotencyToken`). A name is NOT unique: unlike `promptRouterName`, the
//! model documents `inferenceProfileName` as "a name for the inference profile" with no
//! uniqueness clause, so a second create of one name mints a second profile.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateInferenceProfile`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Creates an application inference profile to track metrics and costs when invoking a
//! model. To create an application inference profile for a foundation model in one region,
//! specify the ARN of the model in that region. To create an application inference profile
//! for a foundation model across multiple regions, specify the ARN of the system-defined
//! inference profile that contains the regions that you want to route requests to. For more
//! information, see Increase throughput and resilience with cross-region inference in
//! Amazon Bedrock. in the Amazon Bedrock User Guide.
//!
//! ## Input
//!
//! * `clientRequestToken` (IdempotencyToken, len 1..256, pattern
//!   ^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$, @idempotencyToken) — A unique,
//!   case-sensitive identifier to ensure that the API request completes no more than one
//!   time. If this token matches a previous request, Amazon Bedrock ignores the request, but
//!   does not return an error. For more information, see Ensuring idempotency.
//! * `description` (InferenceProfileDescription, len 1..200, pattern ^([0-9a-zA-Z:.][
//!   _-]?)+$) — A description for the inference profile.
//! * `inferenceProfileName` (InferenceProfileName, len 1..64, pattern ^([0-9a-zA-Z][ _-]?)+$)
//!   **required** — A name for the inference profile.
//! * `modelSource` (InferenceProfileModelSource) **required** — The foundation model or
//!   system-defined inference profile that the inference profile will track metrics and costs
//!   for.
//! * `tags` (TagList, len 0..200) — An array of objects, each of which contains a tag and its
//!   value. For more information, see Tagging resources in the Amazon Bedrock User Guide.
//!
//! ## Output
//!
//! * `inferenceProfileArn` (InferenceProfileArn, len 1..2048, pattern
//!   ^arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:(|[0-9a-z-]{0,20}):(|[0-9]{12}):(inference-profile|application-inference-prof
//!   …) **required** — The ARN of the inference profile that you created.
//! * `status` (InferenceProfileStatus, one of: ACTIVE) — The status of the inference profile.
//!   ACTIVE means that the inference profile is ready to be used.
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
// `CreateInferenceProfileError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateInferenceProfileError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#CreateInferenceProfile")]
#[derive(Debug)]
pub enum CreateInferenceProfileErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    TooManyTagsException(TooManyTagsException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_inference_profile::CreateInferenceProfileError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateInferenceProfileError = crate::scaffolded::Scaffolded<CreateInferenceProfileErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateInferenceProfileErrorModeled> for CreateInferenceProfileError {
    fn from(modeled: CreateInferenceProfileErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateInferenceProfile` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateInferenceProfile<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateInferenceProfile<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateInferenceProfile<Eng>
{
    type Input = CreateInferenceProfileInput;
    type Output = CreateInferenceProfileOutput;
    type Error = CreateInferenceProfileError;

    const NAME: &'static str = "CreateInferenceProfile";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_INFERENCE_PROFILE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_INFERENCE_PROFILE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 201u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(CreateInferenceProfileErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "inference profile state is not available on this engine".to_string(),
                    ),
                },
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();

        let tag_count = input.tags.as_ref().map_or(0, Vec::len);
        if tag_count > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(CreateInferenceProfileErrorModeled::TooManyTagsException(
                TooManyTagsException {
                    message: Some(crate::state::too_many_tags_message()),
                    resource_name: None,
                },
            )
            .into());
        }

        let mut store = state.lock();

        // `@idempotencyToken`: a replay answers with what the first request minted.
        if let Some(minted) = input
            .client_request_token
            .as_ref()
            .and_then(|token| {
                store
                    .inference_profile_create_tokens
                    .get(&(region.clone(), token.clone()))
            })
            .cloned()
        {
            if let Some(existing) = store.inference_profiles.get(&minted) {
                return Ok(CreateInferenceProfileOutput {
                    inference_profile_arn: existing.arn.clone(),
                    status: Some(existing.status.to_string()),
                });
            }
        }

        // The account's inference profile quota. Enforcing it here is also what bounds
        // this part of the store: without a server-side cap a caller could grow it with
        // nothing but fresh creates.
        if store.inference_profiles.len() >= crate::state::MAX_INFERENCE_PROFILES_PER_ACCOUNT {
            return Err(
                CreateInferenceProfileErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: Some(format!(
                            "this account already holds the maximum of {} inference profiles; \
                             delete one or request a quota increase",
                            crate::state::MAX_INFERENCE_PROFILES_PER_ACCOUNT
                        )),
                    },
                )
                .into(),
            );
        }

        let models = tracked_models(&input.model_source, &region);
        let id = store.mint_inference_profile_id(now);
        let arn = crate::state::application_inference_profile_arn(&region, &id);
        let record = crate::state::InferenceProfileRecord {
            arn: arn.clone(),
            id,
            name: input.inference_profile_name,
            description: input.description,
            region: region.clone(),
            models,
            tags: input.tags.unwrap_or_default(),
            status: crate::state::INFERENCE_PROFILE_ACTIVE,
            profile_type: crate::state::INFERENCE_PROFILE_TYPE_APPLICATION,
            created_at: now,
            updated_at: now,
        };

        if let Some(token) = input.client_request_token {
            store
                .inference_profile_create_tokens
                .insert((region, token), arn.clone());
        }
        store.inference_profiles.insert(arn.clone(), record);

        // Read the response back out of the STORED record, so what the caller is told is
        // exactly what a later GetInferenceProfile will find.
        let Some(stored) = store.inference_profiles.get(&arn) else {
            return Err(CreateInferenceProfileErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("the inference profile could not be stored".to_string()),
                },
            )
            .into());
        };
        Ok(CreateInferenceProfileOutput {
            inference_profile_arn: stored.arn.clone(),
            status: Some(stored.status.to_string()),
        })
    }
}

/// The foundation models a profile created from `source` tracks metrics and costs for.
///
/// `InferenceProfileModelSource` carries one member, `copyFrom`, "the ARN of the model or
/// system-defined inference profile that is the source for the inference profile"
/// (`smithy_query(query="shape",shape="InferenceProfileModelSource")`), and its pattern
/// admits a `foundation-model/<id>` or an `inference-profile/<id>` ARN — so the model the
/// profile tracks is the one that source NAMES, read off the ARN's own resource type
/// ([`source_model_id`]).
///
/// The list is rebuilt as a `FoundationModelArn`
/// (`…:bedrock:<region>::foundation-model/<modelId>`,
/// [`crate::state::foundation_model_arn`]) rather than echoing the submitted spelling,
/// whose region and account fields the source's own pattern admits empty; the region is
/// the one the source ARN carries, falling back to the region the request addresses when
/// the source carries none.
///
/// A value `copyFrom` admits that is no ARN of this service at all is tracked as it
/// stands: the member's pattern is enforced ahead of this handler, so there is no such
/// value to read here, and nothing about the request is invented for one.
fn tracked_models(
    source: &InferenceProfileModelSource,
    region: &str,
) -> Vec<InferenceProfileModel> {
    let InferenceProfileModelSource::copyFrom(source_arn) = source;
    let parsed = ::service_common::arn::parse_of(source_arn, crate::state::ARN_NAMESPACE);
    let source_region = parsed
        .map(|arn| arn.region)
        .filter(|field| !field.is_empty())
        .unwrap_or(region);
    vec![InferenceProfileModel {
        model_arn: Some(crate::state::foundation_model_arn(
            source_region,
            source_model_id(source_arn),
        )),
    }]
}

/// The base model id a `copyFrom` source names, read off the resource type its ARN
/// carries.
///
/// A `foundation-model/<modelId>` source names the model directly, so its resource id IS
/// the base model id. An `inference-profile/<id>` source names one of the cross-region
/// profiles Amazon Bedrock publishes, whose id is a base model id behind a geography
/// prefix (`<geography>.<provider>.<model>`), so the model it routes to is what follows
/// that first dot-separated segment.
///
/// The prefix is dropped only while what remains is still a `<provider>.<model>` id —
/// `FoundationModelArn`'s pattern requires at least that much
/// (`[a-z0-9-]{1,63}[.]…[a-z0-9-]{1,63}`) — so an id carrying no prefix to drop is taken
/// as it stands rather than cut down to a value no model ARN could hold.
fn source_model_id(source_arn: &str) -> &str {
    let Some(parsed) = ::service_common::arn::parse_of(source_arn, crate::state::ARN_NAMESPACE)
    else {
        return source_arn;
    };
    let Some((resource_type, id)) = parsed.resource.split_once('/') else {
        return parsed.resource;
    };
    if resource_type != SYSTEM_DEFINED_PROFILE_RESOURCE_TYPE {
        return id;
    }
    match id.split_once('.') {
        Some((_, behind_prefix)) if behind_prefix.contains('.') => behind_prefix,
        _ => id,
    }
}

/// The ARN resource type a system-defined inference profile carries, as `copyFrom`'s
/// pattern spells it beside `foundation-model`
/// (`smithy_query(query="shape",shape="InferenceProfileModelSource")`) — the one the state
/// module states for every surface that reads it.
const SYSTEM_DEFINED_PROFILE_RESOURCE_TYPE: &str = crate::state::INFERENCE_PROFILE_RESOURCE_TYPE;
