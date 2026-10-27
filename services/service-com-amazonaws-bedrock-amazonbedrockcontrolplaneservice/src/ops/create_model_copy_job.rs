// SPDX-License-Identifier: CC0-1.0

//! @op CreateModelCopyJob
//!
//! Submits a copy of one custom model into the region the request addresses, and PERSISTS
//! the job in [`crate::state`]: the `model-copy-job/<id>` ARN the read addresses it by, the
//! source model the request named, the `CustomModelArn` the copy will be minted under, the
//! target name, key and tags, and the creation instant. The ARN the caller is told is read
//! back out of the stored record, so it is the ARN `GetModelCopyJob` answers for.
//!
//! The copy itself runs after this call returns, so the job is stored
//! [`crate::state::MODEL_COPY_JOB_STATUS_IN_PROGRESS`] and owed
//! [`crate::state::MODEL_COPY_JOB_STATUS_COMPLETED`]
//! [`crate::state::MODEL_COPY_JOB_COMPLETE_AFTER`] from now; reaching that status is what
//! registers the copied model among the account's custom models, so a completed copy is a
//! model `GetCustomModel` and `ListCustomModels` report.
//!
//! The rules it enforces, each out of the model or out of what Amazon Bedrock grants
//! rather than out of one request:
//!
//! * `sourceModelArn` names the model to copy, and a `CustomModelArn` EMBEDS the base model
//!   that model was customized from ([`crate::state::custom_model_base_segment`]). A
//!   segment that names no model the published catalog carries
//!   ([`crate::state::FoundationModelCatalog::qualified_model`]) is no foundation model to
//!   copy a model of, so the create is refused synchronously with 400 `ValidationException`
//!   naming that base model ([`crate::state::invalid_base_model_message`]). The
//!   [`crate::state::CUSTOM_MODEL_IMPORTED_BASE`] segment names a model whose artifacts
//!   were brought in, which has no base model in the catalog, so it carries no such
//!   requirement;
//! * a `sourceModelArn` that no custom model of this account answers to is 404
//!   `ResourceNotFoundException`: there is nothing to copy. The source is resolved by its
//!   ARN alone rather than within the region the request addresses, because this operation
//!   "Copies a model to another region", so the source lives in the region ITS OWN ARN
//!   names;
//! * `targetModelName` is the name of the custom model the copy produces, and a model name
//!   names at most one custom model of an account and region, so a name already taken in
//!   the target region is refused with 400 `ValidationException`;
//! * `targetModelTags` past what a resource may carry
//!   ([`crate::state::MAX_TAGS_PER_RESOURCE`]) is refused with 400 `TooManyTagsException`,
//!   the error the model declares for it;
//! * `@idempotencyToken`: a replayed `clientRequestToken` answers with the ARN the first
//!   request minted and copies nothing a second time.
//!
//! A malformed `sourceModelArn` never reaches this handler: the member admits nothing but
//! an ARN, so the service's own ARN check (`crate::service`) refuses one it cannot read
//! ahead of it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateModelCopyJob`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Copies a model to another region so that it can be used there. For more information, see
//! Copy models to be used in other regions in the Amazon Bedrock User Guide.
//!
//! ## Input
//!
//! * `clientRequestToken` (IdempotencyToken, len 1..256, pattern
//!   ^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$, @idempotencyToken) — A unique,
//!   case-sensitive identifier to ensure that the API request completes no more than one
//!   time. If this token matches a previous request, Amazon Bedrock ignores the request, but
//!   does not return an error. For more information, see Ensuring idempotency.
//! * `modelKmsKeyId` (KmsKeyId, len 1..2048, pattern
//!   ^(arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:((key/[a-zA-Z0-9-]{36})|(alias/[a-zA-Z0-9-_/]+)))|([a-zA-Z0-9-]{36})|(ali
//!   …) — The ARN of the KMS key that you use to encrypt the model copy.
//! * `sourceModelArn` (ModelArn, len 20..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:(([0-9]{12}:custom-model/((imported)|([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}))((
//!   …) **required** — The Amazon Resource Name (ARN) of the model to be copied.
//! * `targetModelName` (CustomModelName, len 1..63, pattern ^([0-9a-zA-Z][_-]?){1,63}$)
//!   **required** — A name for the copied model.
//! * `targetModelTags` (TagList, len 0..200) — Tags to associate with the target model. For
//!   more information, see Tag resources in the Amazon Bedrock User Guide.
//!
//! ## Output
//!
//! * `jobArn` (ModelCopyJobArn, len ..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:model-copy-job/[a-z0-9]{12}$)
//!   **required** — The Amazon Resource Name (ARN) of the model copy job.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `TooManyTagsException` (HTTP 400) — The request contains more tags than can be
//!   associated with a resource (50 tags per resource). The maximum number of tags includes
//!   both existing tags and those included in your current request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateModelCopyJobError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateModelCopyJobError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#CreateModelCopyJob")]
#[derive(Debug)]
pub enum CreateModelCopyJobErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    TooManyTagsException(TooManyTagsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_model_copy_job::CreateModelCopyJobError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateModelCopyJobError = crate::scaffolded::Scaffolded<CreateModelCopyJobErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateModelCopyJobErrorModeled> for CreateModelCopyJobError {
    fn from(modeled: CreateModelCopyJobErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateModelCopyJob` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateModelCopyJob<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateModelCopyJob<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateModelCopyJob<Eng>
{
    type Input = CreateModelCopyJobInput;
    type Output = CreateModelCopyJobOutput;
    type Error = CreateModelCopyJobError;

    const NAME: &'static str = "CreateModelCopyJob";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_MODEL_COPY_JOB_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_MODEL_COPY_JOB_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 201u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(CreateModelCopyJobErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("model copy job state is not available on this engine".to_string()),
                },
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();

        let tags = input.target_model_tags.unwrap_or_default();
        if tags.len() > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(CreateModelCopyJobErrorModeled::TooManyTagsException(
                TooManyTagsException {
                    message: Some(crate::state::too_many_tags_message()),
                    resource_name: None,
                },
            )
            .into());
        }

        let mut store = state.lock();

        // `@idempotencyToken`: a replay answers with what the first request minted.
        let replayed = input
            .client_request_token
            .as_ref()
            .and_then(|token| {
                store
                    .model_copy_job_create_tokens
                    .get(&(region.clone(), token.clone()))
            })
            .cloned();
        if let Some(minted) = replayed {
            if store.model_copy_jobs.contains_key(&minted) {
                return Ok(CreateModelCopyJobOutput { job_arn: minted });
            }
        }

        // The base model the source `CustomModelArn` embeds has to be one the published
        // catalog carries: a model customized from a model Amazon Bedrock does not offer is
        // not one it can copy. An imported model is customized from no base model, so it is
        // exempt.
        let base_segment = crate::state::custom_model_base_segment(&input.source_model_arn);
        if let Some(segment) = base_segment {
            if segment != crate::state::CUSTOM_MODEL_IMPORTED_BASE
                && store.foundation_models.qualified_model(segment).is_none()
            {
                return Err(crate::scaffolded::Scaffolded::validation(
                    crate::state::invalid_base_model_message(segment),
                ));
            }
        }

        // The model to copy, as the account holds it. A copy job completing MINTS the
        // copied model, so the source has to be a model that exists — in whichever region
        // its own ARN names, since this operation copies across regions.
        store.settle_custom_models(now);
        let Some(source) = store.custom_models.get(&input.source_model_arn) else {
            return Err(CreateModelCopyJobErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(format!(
                        "no custom model {} exists in this account",
                        input.source_model_arn
                    )),
                },
            )
            .into());
        };
        let source_model_name = source.name.clone();

        // The copied model is a custom model of the TARGET region, so its name is held to
        // the same rule a create of one there is: one name names at most one custom model
        // of an account and region.
        if store
            .custom_model_names
            .contains_key(&(region.clone(), input.target_model_name.clone()))
        {
            return Err(crate::scaffolded::Scaffolded::validation(format!(
                "targetModelName {} is already used by a custom model in this account in \
                 {region}",
                input.target_model_name
            )));
        }

        // The account's copy job bound. Enforcing it here is also what bounds this part of
        // the store: without a server-side cap a caller could grow it with nothing but
        // fresh target model names.
        if store.model_copy_jobs.len() >= crate::state::MAX_MODEL_COPY_JOBS_PER_ACCOUNT {
            return Err(crate::scaffolded::Scaffolded::validation(format!(
                "this account already holds the maximum of {} model copy jobs",
                crate::state::MAX_MODEL_COPY_JOBS_PER_ACCOUNT
            )));
        }

        // The copied model's identity, decided now so the job reports the model it will
        // produce from the call that accepts it: a fresh custom model id of the target
        // region, under the base-model segment the source ARN carries — the copy is of that
        // same customized model, so it descends from the same base.
        let model_id = store.mint_custom_model_id(now);
        let target_model_arn = crate::state::custom_model_arn(
            &region,
            base_segment.unwrap_or(crate::state::CUSTOM_MODEL_IMPORTED_BASE),
            &model_id,
        );
        let id = store.mint_model_copy_job_id(now);
        let arn = crate::state::model_copy_job_arn(&region, &id);
        let record = crate::state::ModelCopyJobRecord {
            arn: arn.clone(),
            region: region.clone(),
            source_model_arn: input.source_model_arn.clone(),
            source_model_name: Some(source_model_name),
            // The account the model being copied originated from: the one its own ARN
            // carries, falling back to this account for a source ARN that names none.
            source_account_id: source_account_id(&input.source_model_arn),
            target_model_arn,
            target_model_name: input.target_model_name.clone(),
            target_model_kms_key_arn: input.model_kms_key_id,
            target_model_tags: tags,
            status: crate::state::MODEL_COPY_JOB_STATUS_IN_PROGRESS,
            completed_due_at: Some(now + crate::state::MODEL_COPY_JOB_COMPLETE_AFTER),
            created_at: now,
        };

        if let Some(token) = input.client_request_token {
            store
                .model_copy_job_create_tokens
                .insert((region, token), arn.clone());
        }
        store.model_copy_jobs.insert(arn.clone(), record);

        // Read the response back out of the STORED record, so what the caller is told is
        // exactly what a later GetModelCopyJob will find.
        let Some(stored) = store.model_copy_jobs.get(&arn) else {
            return Err(CreateModelCopyJobErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("the model copy job could not be stored".to_string()),
                },
            )
            .into());
        };
        Ok(CreateModelCopyJobOutput {
            job_arn: stored.arn.clone(),
        })
    }
}

/// The account a model copy job reports as the one the copied model originated from: the
/// account field of the source model's own ARN.
///
/// `sourceAccountId` is `@required` and modelled `^[0-9]{12}$`, and `ModelArn` admits an
/// EMPTY account field (a `foundation-model` ARN is published by AWS rather than owned by
/// an account), so a source carrying no account reports this emulator's own — the account
/// every resource of this store belongs to.
fn source_account_id(source_model_arn: &str) -> String {
    ::service_common::arn::parse_of(source_model_arn, crate::state::ARN_NAMESPACE)
        .map(|arn| arn.account)
        .filter(|account| {
            account.len() == 12 && account.bytes().all(|digit| digit.is_ascii_digit())
        })
        .unwrap_or(::engine::emulator_account_id())
        .to_string()
}
