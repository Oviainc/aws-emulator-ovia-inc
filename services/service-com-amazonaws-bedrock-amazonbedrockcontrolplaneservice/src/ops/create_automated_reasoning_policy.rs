// SPDX-License-Identifier: CC0-1.0

//! @op CreateAutomatedReasoningPolicy
//!
//! Mints an Automated Reasoning policy and PERSISTS it in [`crate::state`]: the
//! `automated-reasoning-policy/<id>` ARN for the signing region, the request's name,
//! description, policy definition, KMS key and tags, and the creation instant. The policy
//! the caller is told about is read back out of the stored record, so what this answers is
//! exactly what `GetAutomatedReasoningPolicy` finds.
//!
//! The policy starts at its working version — `version` documents "The initial version is
//! always DRAFT" — and carries the `definitionHash` of the definition it was created with
//! ([`crate::state::policy_definition_hash`]), the concurrency token that member is for.
//!
//! A replay of the same `clientRequestToken` answers with the policy that token already
//! minted (`@idempotencyToken`), a name another policy already holds in the same region is
//! refused with `ConflictException`, and a request carrying more tags than one resource
//! may hold is refused with `TooManyTagsException`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateAutomatedReasoningPolicy`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Creates an Automated Reasoning policy for Amazon Bedrock Guardrails. Automated Reasoning
//! policies use mathematical techniques to detect hallucinations, suggest corrections, and
//! highlight unstated assumptions in the responses of your GenAI application. To create a
//! policy, you upload a source document that describes the rules that you're encoding.
//! Automated Reasoning extracts important concepts from the source document that will
//! become variables in the policy and infers policy rules.
//!
//! ## Input
//!
//! * `clientRequestToken` (IdempotencyToken, len 1..256, pattern
//!   ^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$, @idempotencyToken) — A unique,
//!   case-sensitive identifier to ensure that the operation completes no more than once. If
//!   this token matches a previous request, Amazon Bedrock ignores the request but doesn't
//!   return an error.
//! * `description` (AutomatedReasoningPolicyDescription, len ..1024, pattern ^[\s\S]+$) — A
//!   description of the Automated Reasoning policy. Use this to provide context about the
//!   policy's purpose and the types of validations it performs.
//! * `kmsKeyId` (KmsKeyId, len 1..2048, pattern
//!   ^(arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:((key/[a-zA-Z0-9-]{36})|(alias/[a-zA-Z0-9-_/]+)))|([a-zA-Z0-9-]{36})|(ali
//!   …) — The identifier of the KMS key to use for encrypting the automated reasoning policy
//!   and its associated artifacts. If you don't specify a KMS key, Amazon Bedrock uses an KMS
//!   managed key for encryption. For enhanced security and control, you can specify a
//!   customer managed KMS key.
//! * `name` (AutomatedReasoningPolicyName, len 1..256, pattern ^[0-9a-zA-Z-_ ]+$)
//!   **required** — A unique name for the Automated Reasoning policy. The name must be
//!   between 1 and 63 characters and can contain letters, numbers, hyphens, and underscores.
//! * `policyDefinition` (AutomatedReasoningPolicyDefinition) — The policy definition that
//!   contains the formal logic rules, variables, and custom variable types used to validate
//!   foundation model responses in your application.
//! * `tags` (TagList, len 0..200) — A list of tags to associate with the Automated Reasoning
//!   policy. Tags help you organize and manage your policies.
//!
//! ## Output
//!
//! * `createdAt` (Timestamp) **required** — The timestamp when the policy was created.
//! * `definitionHash` (AutomatedReasoningPolicyHash, len 128..128, pattern ^[0-9a-z]{128}$) —
//!   The hash of the policy definition. This is used as a concurrency token for creating
//!   policy versions that you can use in your application.
//! * `description` (AutomatedReasoningPolicyDescription, len ..1024, pattern ^[\s\S]+$) — The
//!   description of the Automated Reasoning policy.
//! * `name` (AutomatedReasoningPolicyName, len 1..256, pattern ^[0-9a-zA-Z-_ ]+$)
//!   **required** — The name of the Automated Reasoning policy.
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy that you
//!   created.
//! * `updatedAt` (Timestamp) **required** — The timestamp when the policy was last updated.
//! * `version` (AutomatedReasoningPolicyVersion, len ..12, pattern ^([1-9][0-9]{0,11})$)
//!   **required** — The version number of the newly created Automated Reasoning policy. The
//!   initial version is always DRAFT.
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
// `CreateAutomatedReasoningPolicyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateAutomatedReasoningPolicyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#CreateAutomatedReasoningPolicy")]
#[derive(Debug)]
pub enum CreateAutomatedReasoningPolicyErrorModeled {
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
/// (`crate::ops::create_automated_reasoning_policy::CreateAutomatedReasoningPolicyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateAutomatedReasoningPolicyError = crate::scaffolded::Scaffolded<CreateAutomatedReasoningPolicyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateAutomatedReasoningPolicyErrorModeled> for CreateAutomatedReasoningPolicyError {
    fn from(modeled: CreateAutomatedReasoningPolicyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateAutomatedReasoningPolicy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateAutomatedReasoningPolicy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateAutomatedReasoningPolicy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateAutomatedReasoningPolicy<Eng>
{
    type Input = CreateAutomatedReasoningPolicyInput;
    type Output = CreateAutomatedReasoningPolicyOutput;
    type Error = CreateAutomatedReasoningPolicyError;

    const NAME: &'static str = "CreateAutomatedReasoningPolicy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_AUTOMATED_REASONING_POLICY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_AUTOMATED_REASONING_POLICY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(server_error());
        };
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();

        // `TagList` admits up to 200 entries, while one RESOURCE may carry at most
        // `MAX_TAGS_PER_RESOURCE` — the bound `TooManyTagsException` itself documents —
        // so a request past it is refused rather than stored.
        let tag_count = input.tags.as_ref().map_or(0, Vec::len);
        if tag_count > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(
                CreateAutomatedReasoningPolicyErrorModeled::TooManyTagsException(
                    TooManyTagsException {
                        message: Some(format!(
                            "a resource may carry at most {} tags, and this request carries \
                             {tag_count}",
                            crate::state::MAX_TAGS_PER_RESOURCE
                        )),
                        resource_name: None,
                    },
                )
                .into(),
            );
        }

        let mut store = state.lock();

        // `@idempotencyToken`: a replay answers with what the first request minted.
        if let Some(minted) = input
            .client_request_token
            .as_ref()
            .and_then(|token| {
                store
                    .automated_reasoning_policy_create_tokens
                    .get(&(region.clone(), token.clone()))
            })
            .cloned()
        {
            if let Some(existing) = store.automated_reasoning_policy(&minted) {
                return Ok(created(existing));
            }
        }

        let name_key = (region.clone(), input.name.clone());
        if store.automated_reasoning_policy_names.contains_key(&name_key) {
            return Err(
                CreateAutomatedReasoningPolicyErrorModeled::ConflictException(ConflictException {
                    message: Some(format!(
                        "an automated reasoning policy named {} already exists in this \
                         account in {region}",
                        input.name
                    )),
                })
                .into(),
            );
        }

        // The account's policy quota. Enforcing it here is also what bounds this part of
        // the store: without a server-side cap a caller could grow it with nothing but
        // fresh names.
        if store.automated_reasoning_policies.len()
            >= crate::state::MAX_AUTOMATED_REASONING_POLICIES_PER_ACCOUNT
        {
            return Err(
                CreateAutomatedReasoningPolicyErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: Some(format!(
                            "this account already holds the maximum of {} automated reasoning \
                             policies; delete one or request a quota increase",
                            crate::state::MAX_AUTOMATED_REASONING_POLICIES_PER_ACCOUNT
                        )),
                    },
                )
                .into(),
            );
        }

        let id = store.mint_automated_reasoning_policy_id(now);
        let arn = crate::state::automated_reasoning_policy_arn(&region, &id);
        let record = crate::state::AutomatedReasoningPolicyRecord {
            arn: arn.clone(),
            id,
            name: input.name,
            region: region.clone(),
            definition_hash: crate::state::policy_definition_hash(input.policy_definition.as_ref()),
            definition: input.policy_definition,
            description: input.description,
            kms_key_arn: crate::state::kms_key_arn(input.kms_key_id.as_deref(), &region),
            tags: input.tags.unwrap_or_default(),
            created_at: now,
            updated_at: now,
        };

        store
            .automated_reasoning_policy_names
            .insert(name_key, arn.clone());
        if let Some(token) = input.client_request_token {
            store
                .automated_reasoning_policy_create_tokens
                .insert((region, token), arn.clone());
        }
        store.automated_reasoning_policies.insert(arn.clone(), record);

        // Read the response back out of the STORED record, so what the caller is told is
        // exactly what a later GetAutomatedReasoningPolicy will find. A record that is not
        // there is answered as a server error rather than by unwinding this request.
        let Some(stored) = store.automated_reasoning_policy(&arn) else {
            return Err(server_error());
        };
        Ok(created(stored))
    }
}

/// What this operation answers for a stored policy, so the create and its idempotent
/// replay report ONE record the same way.
fn created(
    record: &crate::state::AutomatedReasoningPolicyRecord,
) -> CreateAutomatedReasoningPolicyOutput {
    CreateAutomatedReasoningPolicyOutput {
        policy_arn: record.arn.clone(),
        // The policy's working version: `version` documents "The initial version is always
        // DRAFT", so a freshly created policy reports the draft rather than a number.
        version: crate::state::DRAFT_VERSION.to_string(),
        name: record.name.clone(),
        description: record.description.clone(),
        definition_hash: Some(record.definition_hash.clone()),
        created_at: crate::state::date_time(record.created_at),
        updated_at: crate::state::date_time(record.updated_at),
    }
}

/// The modelled 500 this operation answers when the engine carries no store for this
/// service — never a fresh engine, which would drop the policy on return.
fn server_error() -> CreateAutomatedReasoningPolicyError {
    CreateAutomatedReasoningPolicyErrorModeled::InternalServerException(InternalServerException {
        message: Some("automated reasoning policy state is not available on this engine".to_string()),
    })
    .into()
}
