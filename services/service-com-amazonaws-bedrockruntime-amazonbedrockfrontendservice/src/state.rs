// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrockruntime-amazonbedrockfrontendservice service.
//!
//! Besides the state struct itself this module holds the pieces every Bedrock
//! Runtime handler shares, so no two operations grow their own copy:
//!
//! * the seeded **model inventory** (`models`) that `modelId` resolution and
//!   its refusals are driven from — a 400 `ValidationException` for an
//!   identifier the service cannot interpret, a 404
//!   `ResourceNotFoundException` for one naming a model or resource that is not
//!   available here;
//! * the **tokenizer** (`count_text_tokens`, `truncate_to_tokens`) that
//!   `CountTokens`, `Converse` and `InvokeModel` all count with, so the counts
//!   two operations report for the same text agree by construction;
//! * the **prompt-text extraction** rule used to count tokens in an
//!   `InvokeModel` body;
//! * the **async-invoke** records, their idempotency index and the clock-owed
//!   transition that moves a submitted invocation out of `InProgress`.
//!
//! No language model runs inside this emulator, so nothing here invents model
//! output and presents it as inference: a synchronous completion carries, in the
//! model family's own response envelope, text that says plainly that no model
//! ran, together with the token accounting computed from the real request; an
//! asynchronous invocation ends in the service's own `Failed` state with a
//! failure message that says the same (see [`ASYNC_INVOKE_FAILURE_MESSAGE`]).
//! Everything around that — resolution, validation, status, timestamps, ARNs,
//! paging and the invocation log — is real and computed from persisted state.

use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::ops::types::{
    AsyncInvokeOutputDataConfig, ContentBlock, Message, SystemContentBlock, Tag, Tool,
    ToolConfiguration,
};

/// The most async-invoke records this emulator retains.
///
/// AWS publishes no retention for them, so this is the EMULATOR's bound, and it
/// exists so a caller cannot grow the store without limit: the oldest record is
/// evicted (with its idempotency-index entry) once the store passes it. It is
/// far above any test or client working set, so eviction never hides a record a
/// caller is still polling.
pub const MAX_ASYNC_INVOKE_RECORDS: usize = 1000;

/// The most inference-log entries this emulator retains: the log is a ring
/// buffer of the most recent calls, so one entry per request cannot grow without
/// limit either.
pub const MAX_INVOCATION_LOG_ENTRIES: usize = 1000;

/// The page size `ListAsyncInvokes` uses when the caller names no `maxResults`,
/// and the ceiling it clamps one to: the modelled `maxResults` maximum, so one
/// request cannot render an unbounded number of records into one response.
pub const MAX_PAGE_SIZE: usize = 1000;

/// How deep a caller-supplied document or body is walked before the walk stops.
///
/// Keeps the recursion bound LOCAL to this crate rather than inherited from a
/// deserializer's own limit.
pub const MAX_DOCUMENT_DEPTH: usize = 64;

/// Per-service state for com-amazonaws-bedrockruntime-amazonbedrockfrontendservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState {
    /// The recognised foundation models, keyed by model id. Seeded by
    /// [`Default`], so `modelId` resolution and its refusals are state-driven.
    pub models: BTreeMap<String, ModelEntry>,
    /// Every async invocation this emulator still retains, keyed by
    /// `invocationArn`. Bounded by [`MAX_ASYNC_INVOKE_RECORDS`].
    pub async_invokes: BTreeMap<String, AsyncInvokeRecord>,
    /// `clientRequestToken` -> `invocationArn`, so `StartAsyncInvoke` is
    /// idempotent and can detect a reused token carrying a different payload.
    /// An entry is dropped with the record it points at, so this index cannot
    /// outgrow `async_invokes`.
    pub idempotency_index: HashMap<String, String>,
    /// The inference log: one entry per served inference call, as a ring buffer
    /// of the most recent [`MAX_INVOCATION_LOG_ENTRIES`].
    pub invocations: VecDeque<InvocationLogEntry>,
    /// Monotonic counter mixed into every minted async-invoke id.
    pub next_id: u64,
}

impl Default for ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState {
    fn default() -> Self {
        let mut models = BTreeMap::new();
        for entry in seed_models() {
            models.insert(entry.model_id.clone(), entry);
        }
        Self {
            models,
            async_invokes: BTreeMap::new(),
            idempotency_index: HashMap::new(),
            invocations: VecDeque::new(),
            next_id: 0,
        }
    }
}

impl ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState {
    /// Resolve a `modelId` as given on the wire to a registry entry, or `None`
    /// when this emulator does not recognise the model.
    ///
    /// Accepts the spellings the model's `modelId` pattern allows for a base
    /// model: the bare model id, a `foundation-model/<id>` ARN, an
    /// `inference-profile/<id>` ARN, and a cross-region inference profile id,
    /// which prefixes the model id with a geography (`us.`, `eu.`, `apac.`,
    /// `us-gov.`) — see "Supported Regions and models for cross-region
    /// inference" in the Amazon Bedrock User Guide.
    ///
    /// This is the raw registry lookup: it says nothing about whether the model
    /// can serve inference. An operation that is about to invoke a model calls
    /// [`Self::resolve_model_for_inference`] instead, which carries the refusal
    /// the service owes for an identifier it cannot serve.
    pub fn resolve_model(&self, model_id: &str) -> Option<&ModelEntry> {
        match interpret_model_id(model_id) {
            ModelIdentifier::BaseModel { key, .. } => self.models.get(key.as_str()),
            ModelIdentifier::AccountResource => None,
        }
    }

    /// Resolve a `modelId` for a call that is about to run inference against it.
    ///
    /// The identifier is interpreted BEFORE the inventory is consulted, because
    /// the two failures are different faults with different statuses:
    ///
    /// * an identifier this service cannot interpret as naming a model it offers
    ///   — a bare id (or cross-region profile id) that is in no inventory — is a
    ///   client input fault: [`ModelRejection::Invalid`], answered with
    ///   `ValidationException` (400) and the service's fixed
    ///   [`INVALID_MODEL_IDENTIFIER_MESSAGE`];
    /// * an identifier that DOES name a resource — an ARN, or a base model the
    ///   inventory carries as retired — whose resource is not available here is
    ///   [`ModelRejection::NotFound`], answered with `ResourceNotFoundException`
    ///   (404).
    ///
    /// Keeping the two apart is what lets an SDK classify the failure: 400 is a
    /// non-retryable client fault, while 404 is routinely retried as
    /// eventual consistency.
    pub fn resolve_model_for_inference(&self, model_id: &str) -> Result<&ModelEntry, ModelRejection> {
        let (key, named_by_arn) = match interpret_model_id(model_id) {
            ModelIdentifier::BaseModel { key, named_by_arn } => (key, named_by_arn),
            // A well-formed ARN of an account-scoped resource: a custom model, a
            // provisioned throughput, an imported model, a custom-model
            // deployment, a prompt, a prompt router or a SageMaker endpoint.
            // This emulator holds no such resource, so the ARN names one that
            // does not exist.
            ModelIdentifier::AccountResource => {
                return Err(ModelRejection::NotFound(format!(
                    "Could not resolve the resource named by the provided model identifier: \
                     {model_id}"
                )))
            }
        };
        match self.models.get(key.as_str()) {
            Some(entry) if entry.lifecycle == ModelLifecycle::EndOfLife => {
                Err(ModelRejection::NotFound(END_OF_LIFE_MESSAGE.to_string()))
            }
            Some(entry) => Ok(entry),
            None if named_by_arn => Err(ModelRejection::NotFound(format!(
                "Could not resolve the model named by the provided model identifier: {model_id}"
            ))),
            None => Err(ModelRejection::Invalid(
                INVALID_MODEL_IDENTIFIER_MESSAGE.to_string(),
            )),
        }
    }

    /// The next async-invoke id: 12 lowercase alphanumeric characters, as the
    /// `InvocationArn` pattern requires.
    ///
    /// Derived from the state counter plus the request's own values, so two
    /// invocations never collide and the id is not a guessable sequence.
    pub fn mint_invocation_id(&mut self, model_id: &str, submit_millis: i64) -> String {
        use sha2::{Digest, Sha256};
        self.next_id += 1;
        let seed = format!("{}:{}:{}", self.next_id, model_id, submit_millis);
        let digest = Sha256::digest(seed.as_bytes());
        hex::encode(digest)[..12].to_string()
    }

    /// Materialise every async-invoke transition the clock has already made due
    /// at `now_millis`.
    ///
    /// A reporting operation calls this before it answers: the change was owed
    /// at this instant whether or not anybody called, so applying it is not a
    /// read with a side effect.
    pub fn settle_async_invokes(&mut self, now_millis: i64) {
        for record in self.async_invokes.values_mut() {
            if record.status == AsyncInvokeStatus::InProgress && now_millis >= record.settles_at {
                record.status = AsyncInvokeStatus::Failed;
                record.failure_message = Some(ASYNC_INVOKE_FAILURE_MESSAGE.to_string());
                record.end_time = Some(record.settles_at);
                record.last_modified_time = Some(record.settles_at);
            }
        }
    }

    /// Persist a submitted invocation, index it under its idempotency token, and
    /// evict the oldest records once the store passes
    /// [`MAX_ASYNC_INVOKE_RECORDS`].
    pub fn insert_async_invoke(&mut self, record: AsyncInvokeRecord) {
        let invocation_arn = record.invocation_arn.clone();
        if let Some(token) = record.client_request_token.clone() {
            self.idempotency_index.insert(token, invocation_arn.clone());
        }
        self.async_invokes.insert(invocation_arn, record);
        while self.async_invokes.len() > MAX_ASYNC_INVOKE_RECORDS {
            let oldest = self
                .async_invokes
                .values()
                .min_by(|left, right| {
                    (left.submit_time, left.invocation_arn.as_str())
                        .cmp(&(right.submit_time, right.invocation_arn.as_str()))
                })
                .map(|record| record.invocation_arn.clone());
            let Some(oldest) = oldest else { break };
            let evicted = self.async_invokes.remove(&oldest);
            // Drop the index entry only while it still points at the evicted
            // record: a later invocation may have taken the same token over.
            if let Some(token) = evicted.and_then(|record| record.client_request_token) {
                if self
                    .idempotency_index
                    .get(&token)
                    .is_some_and(|arn| *arn == oldest)
                {
                    self.idempotency_index.remove(&token);
                }
            }
        }
    }

    /// Record one served inference call, keeping the log at most
    /// [`MAX_INVOCATION_LOG_ENTRIES`] entries long.
    pub fn log_invocation(&mut self, entry: InvocationLogEntry) {
        self.invocations.push_back(entry);
        while self.invocations.len() > MAX_INVOCATION_LOG_ENTRIES {
            self.invocations.pop_front();
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState>)
}

// ---------------------------------------------------------------------------
// The model registry
// ---------------------------------------------------------------------------

/// The request/response body family a base model speaks on `InvokeModel`.
///
/// One variant per request/response envelope documented in "Inference request
/// parameters and response fields for foundation models" in the Amazon Bedrock
/// User Guide; the envelope each one emits is in
/// [`crate::state::invoke_model_response_body`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFamily {
    /// Anthropic Claude (Messages API).
    AnthropicMessages,
    /// Amazon Titan Text.
    TitanText,
    /// Amazon Nova (Converse-shaped body).
    Nova,
    /// Meta Llama.
    Llama,
    /// Mistral AI.
    Mistral,
    /// Cohere Command.
    CohereCommand,
}

/// What a base model takes and produces.
///
/// Modalities as published per model in "Supported foundation models in Amazon
/// Bedrock" in the Amazon Bedrock User Guide. Only [`ModelModality::Text`] has
/// an `InvokeModel` envelope in this emulator; an invocation of any other
/// modality is refused with the modelled error rather than answered with a
/// fabricated embedding, image or video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelModality {
    /// Text in, text out.
    Text,
    /// Text in, an embedding vector out.
    Embedding,
    /// An image out.
    Image,
    /// A video out.
    Video,
}

impl ModelModality {
    /// The modality as it reads in a refusal message.
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Embedding => "embedding",
            Self::Image => "image",
            Self::Video => "video",
        }
    }
}

/// Where a base model is in the Bedrock model lifecycle.
///
/// "Model lifecycle" in the Amazon Bedrock User Guide: a model id stays a
/// recognised identifier after its model reaches end of life, but the model no
/// longer serves inference — which is a 404 on a resource that existed, not a
/// 400 on an identifier the service cannot read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelLifecycle {
    /// Available for inference.
    Active,
    /// Past its end-of-life date: the identifier resolves, the model is gone.
    EndOfLife,
}

/// One foundation model id this service recognises.
#[derive(Debug, Clone)]
pub struct ModelEntry {
    /// The bare model id, e.g. `anthropic.claude-3-sonnet-20240229-v1:0`.
    pub model_id: String,
    /// Which `InvokeModel` body envelope the model speaks, or `None` when this
    /// emulator implements none for it — every non-text modality, and the text
    /// providers whose envelope it does not render (see [`seed_models`]). An
    /// invocation of one is refused, never answered with an invented payload.
    pub family: Option<ModelFamily>,
    /// What the model takes and produces.
    pub modality: ModelModality,
    /// Whether the model still serves inference at all.
    pub lifecycle: ModelLifecycle,
    /// Whether the model is served by the Converse API. Not every base model is
    /// — "Supported models and model features" in the Amazon Bedrock User Guide
    /// lists the Cohere Command (text) models as `InvokeModel`-only, for
    /// instance — and `Converse` must refuse one that is not.
    pub supports_converse: bool,
    /// The model's own ceiling on generated tokens, which caps
    /// `inferenceConfig.maxTokens`. Zero for a model that generates no text.
    pub max_output_tokens: u32,
}

/// The foundation model ids this service recognises.
///
/// Ids as published in "Amazon Bedrock base model IDs (on-demand throughput)"
/// and "Supported foundation models in Amazon Bedrock" in the Amazon Bedrock
/// User Guide, and the retired ones in "Model lifecycle". The inventory carries
/// an id whether or not this emulator can serve it, because resolving the
/// IDENTIFIER and serving the MODEL are separate steps: a recognised id is
/// never reported as an invalid identifier.
///
/// It covers, for every provider whose `InvokeModel` envelope
/// [`invoke_model_response_body`] renders (Amazon Titan and Nova, Anthropic
/// Claude, Meta Llama, Mistral AI and Cohere Command), that provider's
/// published on-demand base models — text models served in their own envelope,
/// non-text models recognised but refused — plus the retired ids of every
/// provider, which need no envelope. A text provider whose envelope is not
/// implemented (AI21 Jamba, DeepSeek, Writer, Qwen) is NOT listed, so its id
/// is answered as an identifier this emulator cannot interpret.
fn seed_models() -> Vec<ModelEntry> {
    // A text model served in its family's envelope.
    let text = |model_id: &str, family: ModelFamily, supports_converse: bool, max: u32| ModelEntry {
        model_id: model_id.to_string(),
        family: Some(family),
        modality: ModelModality::Text,
        lifecycle: ModelLifecycle::Active,
        supports_converse,
        max_output_tokens: max,
    };
    // A recognised model whose modality this emulator computes nothing for.
    let unserved = |model_id: &str, modality: ModelModality| ModelEntry {
        model_id: model_id.to_string(),
        family: None,
        modality,
        lifecycle: ModelLifecycle::Active,
        supports_converse: false,
        max_output_tokens: 0,
    };
    // A model past its end of life: recognised, not servable.
    let retired = |model_id: &str, modality: ModelModality| ModelEntry {
        model_id: model_id.to_string(),
        family: None,
        modality,
        lifecycle: ModelLifecycle::EndOfLife,
        supports_converse: false,
        max_output_tokens: 0,
    };
    use ModelFamily::{AnthropicMessages, CohereCommand, Llama, Mistral, Nova, TitanText};
    use ModelModality::{Embedding, Image, Text, Video};
    vec![
        // Anthropic Claude — 4k max output tokens on Claude 3, 8k on 3.5 and
        // 3.7 ("Inference parameters for foundation models", Anthropic Claude).
        text("anthropic.claude-3-haiku-20240307-v1:0", AnthropicMessages, true, 4096),
        text("anthropic.claude-3-sonnet-20240229-v1:0", AnthropicMessages, true, 4096),
        text("anthropic.claude-3-opus-20240229-v1:0", AnthropicMessages, true, 4096),
        text("anthropic.claude-3-5-haiku-20241022-v1:0", AnthropicMessages, true, 8192),
        text("anthropic.claude-3-5-sonnet-20240620-v1:0", AnthropicMessages, true, 8192),
        text("anthropic.claude-3-5-sonnet-20241022-v2:0", AnthropicMessages, true, 8192),
        text("anthropic.claude-3-7-sonnet-20250219-v1:0", AnthropicMessages, true, 8192),
        // Amazon Titan Text.
        text("amazon.titan-text-lite-v1", TitanText, true, 4096),
        text("amazon.titan-text-express-v1", TitanText, true, 8192),
        text("amazon.titan-text-premier-v1:0", TitanText, true, 3072),
        // Amazon Nova understanding models — 5k max output tokens.
        text("amazon.nova-micro-v1:0", Nova, true, 5120),
        text("amazon.nova-lite-v1:0", Nova, true, 5120),
        text("amazon.nova-pro-v1:0", Nova, true, 5120),
        text("amazon.nova-premier-v1:0", Nova, true, 5120),
        // Meta Llama 3 and later — 2k max generation length.
        text("meta.llama3-8b-instruct-v1:0", Llama, true, 2048),
        text("meta.llama3-70b-instruct-v1:0", Llama, true, 2048),
        text("meta.llama3-1-8b-instruct-v1:0", Llama, true, 2048),
        text("meta.llama3-1-70b-instruct-v1:0", Llama, true, 2048),
        text("meta.llama3-1-405b-instruct-v1:0", Llama, true, 2048),
        text("meta.llama3-2-1b-instruct-v1:0", Llama, true, 2048),
        text("meta.llama3-2-3b-instruct-v1:0", Llama, true, 2048),
        text("meta.llama3-2-11b-instruct-v1:0", Llama, true, 2048),
        text("meta.llama3-2-90b-instruct-v1:0", Llama, true, 2048),
        text("meta.llama3-3-70b-instruct-v1:0", Llama, true, 2048),
        // Mistral AI.
        text("mistral.mistral-7b-instruct-v0:2", Mistral, true, 8192),
        text("mistral.mixtral-8x7b-instruct-v0:1", Mistral, true, 4096),
        text("mistral.mistral-small-2402-v1:0", Mistral, true, 8192),
        text("mistral.mistral-large-2402-v1:0", Mistral, true, 8192),
        text("mistral.mistral-large-2407-v1:0", Mistral, true, 8192),
        // Cohere Command: the text models are InvokeModel-only, the R models
        // are served by Converse too.
        text("cohere.command-text-v14", CohereCommand, false, 4096),
        text("cohere.command-light-text-v14", CohereCommand, false, 4096),
        text("cohere.command-r-v1:0", CohereCommand, true, 4096),
        text("cohere.command-r-plus-v1:0", CohereCommand, true, 4096),
        // Recognised ids whose modality this emulator serves no inference for.
        unserved("amazon.titan-embed-text-v1", Embedding),
        unserved("amazon.titan-embed-text-v2:0", Embedding),
        unserved("amazon.titan-embed-image-v1", Embedding),
        unserved("cohere.embed-english-v3", Embedding),
        unserved("cohere.embed-multilingual-v3", Embedding),
        unserved("amazon.titan-image-generator-v1", Image),
        unserved("amazon.titan-image-generator-v2:0", Image),
        unserved("amazon.nova-canvas-v1:0", Image),
        unserved("amazon.nova-reel-v1:0", Video),
        unserved("amazon.nova-reel-v1:1", Video),
        // Past end of life ("Model lifecycle").
        retired("anthropic.claude-v1", Text),
        retired("anthropic.claude-v2", Text),
        retired("anthropic.claude-v2:1", Text),
        retired("anthropic.claude-instant-v1", Text),
        retired("ai21.j2-mid-v1", Text),
        retired("ai21.j2-ultra-v1", Text),
        retired("ai21.j2-grande-instruct", Text),
        retired("ai21.j2-jumbo-instruct", Text),
        retired("meta.llama2-13b-chat-v1", Text),
        retired("meta.llama2-70b-chat-v1", Text),
        retired("stability.stable-diffusion-xl-v0", Image),
    ]
}

/// The service's fixed message for a `modelId` it cannot interpret as naming a
/// model it offers.
pub const INVALID_MODEL_IDENTIFIER_MESSAGE: &str = "The provided model identifier is invalid.";

/// The service's message for a model id whose model has reached end of life
/// ("Model lifecycle" in the Amazon Bedrock User Guide).
pub const END_OF_LIFE_MESSAGE: &str = "This model version has reached the end of its life. \
                                       Please refer to the AWS documentation for more details.";

/// Why a `modelId` cannot be served, as the modelled error the refusing
/// operation answers with. See
/// [`ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState::resolve_model_for_inference`].
#[derive(Debug, Clone)]
pub enum ModelRejection {
    /// Not an identifier this service can interpret as naming a model it
    /// offers: `ValidationException`, HTTP 400.
    Invalid(String),
    /// It names a model or a resource that is not available here:
    /// `ResourceNotFoundException`, HTTP 404.
    NotFound(String),
}

/// Geography prefixes a cross-region inference profile id puts in front of the
/// base model id. Source: "Supported Regions and models for cross-region
/// inference" in the Amazon Bedrock User Guide.
const INFERENCE_PROFILE_PREFIXES: [&str; 4] = ["us.", "eu.", "apac.", "us-gov."];

/// The ARN resource prefixes of the inference resources a `modelId` may name
/// that are NOT a base model, from the `InvokeModelIdentifier` pattern:
/// `custom-model`, `imported-model`, `provisioned-model`,
/// `custom-model-deployment`, `prompt`, `prompt-router`,
/// `default-prompt-router`, and a SageMaker `endpoint`. Each is created in an
/// account, so an ARN naming one this emulator has no record of names an absent
/// resource rather than an unreadable identifier.
const ACCOUNT_RESOURCE_SEGMENTS: [&str; 8] = [
    ":custom-model/",
    ":imported-model/",
    ":provisioned-model/",
    ":custom-model-deployment/",
    ":prompt/",
    ":prompt-router/",
    ":default-prompt-router/",
    ":endpoint/",
];

/// What a `modelId` as given on the wire names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ModelIdentifier {
    /// A base model, whose inventory key is `key`. `named_by_arn` records that
    /// the caller named it by ARN (a `foundation-model` or `inference-profile`
    /// ARN), which is a resource reference: when it resolves to nothing, the
    /// resource is absent rather than the identifier unreadable.
    BaseModel { key: String, named_by_arn: bool },
    /// An ARN of an account-scoped inference resource.
    AccountResource,
}

/// Interpret a `modelId` as given on the wire, without consulting the
/// inventory.
fn interpret_model_id(model_id: &str) -> ModelIdentifier {
    let (bare, named_by_arn) = if model_id.starts_with("arn:") {
        if ACCOUNT_RESOURCE_SEGMENTS
            .iter()
            .any(|segment| model_id.contains(segment))
        {
            return ModelIdentifier::AccountResource;
        }
        // A foundation-model ARN, or an inference-profile ARN whose resource id
        // is the (possibly geography-prefixed) model id.
        match ["foundation-model/", "inference-profile/"]
            .iter()
            .find_map(|segment| model_id.split_once(segment))
        {
            Some((_, resource)) => (resource, true),
            None => return ModelIdentifier::AccountResource,
        }
    } else {
        (model_id, false)
    };
    // A cross-region inference profile id is the base model id behind a
    // geography prefix, and resolves to the same model.
    let key = INFERENCE_PROFILE_PREFIXES
        .iter()
        .find_map(|prefix| bare.strip_prefix(prefix))
        .unwrap_or(bare);
    ModelIdentifier::BaseModel {
        key: key.to_string(),
        named_by_arn,
    }
}

/// The `foundation-model` ARN of a base model in `region`, as `GetAsyncInvoke`
/// reports it in `modelArn`.
pub fn foundation_model_arn(region: &str, model_id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("foundation-model/{model_id}"))
}

/// The region an unsigned (anonymous) request is served in.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The AWS region this request was signed for, defaulting to [`DEFAULT_REGION`]
/// when the request carries no SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

// ---------------------------------------------------------------------------
// The inference log
// ---------------------------------------------------------------------------

/// Which API served an inference call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvokeKind {
    /// `InvokeModel`.
    Sync,
    /// `StartAsyncInvoke`.
    Async,
    /// `Converse`.
    Converse,
}

/// One served inference call.
#[derive(Debug, Clone)]
pub struct InvocationLogEntry {
    pub model_id: String,
    pub kind: InvokeKind,
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// Epoch milliseconds at which the call was served.
    pub at_millis: i64,
}

// ---------------------------------------------------------------------------
// Async invocations
// ---------------------------------------------------------------------------

/// The lifecycle status of an async invocation, as modelled by
/// `AsyncInvokeStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncInvokeStatus {
    InProgress,
    Completed,
    Failed,
}

impl AsyncInvokeStatus {
    /// The modelled enum value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "InProgress",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
        }
    }

    /// Parse a modelled enum value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "InProgress" => Some(Self::InProgress),
            "Completed" => Some(Self::Completed),
            "Failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// How long after `StartAsyncInvoke` the invocation leaves `InProgress`.
///
/// AWS documents no duration for this, so the span is CHOSEN: half a second,
/// inside this project's 250–750 ms window for a clock-owed transition, so a
/// caller that reads the record straight after submitting it still observes
/// `InProgress` and the state errors that go with it. The due instant is stored
/// on the record ([`AsyncInvokeRecord::settles_at`]) and applied when a
/// reporting operation next runs; nothing sleeps.
pub const ASYNC_INVOKE_SETTLE_MILLIS: i64 = 500;

/// Why a submitted async invocation ends in `Failed`.
///
/// No model runs inside this emulator, and an asynchronous invocation's only
/// result is the artifact the service writes to the caller's S3 location, so
/// there is nothing honest to report as `Completed`: the invocation ends in the
/// service's own modelled failure state with a reason that says why.
pub const ASYNC_INVOKE_FAILURE_MESSAGE: &str =
    "This Amazon Bedrock Runtime emulator does not run model inference for asynchronous \
     invocations, so no output was written to the configured S3 location.";

/// One async invocation, exactly as `StartAsyncInvoke` stored it.
#[derive(Debug, Clone)]
pub struct AsyncInvokeRecord {
    pub invocation_arn: String,
    pub model_id: String,
    pub model_arn: String,
    pub client_request_token: Option<String>,
    pub status: AsyncInvokeStatus,
    /// Epoch milliseconds of the submitting call.
    pub submit_time: i64,
    pub last_modified_time: Option<i64>,
    pub end_time: Option<i64>,
    pub failure_message: Option<String>,
    pub output_data_config: AsyncInvokeOutputDataConfig,
    pub model_input: ::aws_smithy_types::Document,
    pub tags: Vec<Tag>,
    /// Epoch milliseconds at which this record leaves `InProgress`.
    pub settles_at: i64,
    /// Digest of the submitted payload, compared when a `clientRequestToken` is
    /// reused. A fixed-size digest rather than the canonical payload itself, so
    /// the record keeps no second copy of the caller's data.
    pub request_fingerprint: String,
}

/// The fingerprint of a `StartAsyncInvoke` payload: a SHA-256 digest of its
/// canonical JSON rendering, used to tell an idempotent retry from a conflicting
/// reuse of the same token.
pub fn async_invoke_fingerprint(
    model_id: &str,
    model_input: &::aws_smithy_types::Document,
    output_data_config: &AsyncInvokeOutputDataConfig,
    tags: &[Tag],
) -> String {
    use sha2::{Digest, Sha256};
    let output = match output_data_config {
        AsyncInvokeOutputDataConfig::s3OutputDataConfig(config) => serde_json::json!({
            "s3Uri": config.s3_uri,
            "kmsKeyId": config.kms_key_id,
            "bucketOwner": config.bucket_owner,
        }),
    };
    let mut tag_pairs: Vec<(&str, &str)> = tags
        .iter()
        .map(|tag| (tag.key.as_str(), tag.value.as_str()))
        .collect();
    tag_pairs.sort_unstable();
    // `serde_json::Map` is ordered, so this rendering is canonical.
    let canonical = serde_json::json!({
        "modelId": model_id,
        "modelInput": document_to_json(model_input),
        "outputDataConfig": output,
        "tags": tag_pairs,
    })
    .to_string();
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

/// A page token for `ListAsyncInvokes`: the sort key of the last item of the
/// page just returned, hex-encoded so it is opaque to the caller and matches the
/// modelled `^\S*$` token pattern.
pub fn encode_page_token(submit_time: i64, invocation_arn: &str) -> String {
    hex::encode(format!("{submit_time}|{invocation_arn}"))
}

/// The sort key a page token carries, or `None` when the token is not one this
/// emulator minted.
pub fn decode_page_token(token: &str) -> Option<(i64, String)> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (submit_time, arn) = decoded.split_once('|')?;
    Some((submit_time.parse().ok()?, arn.to_string()))
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// Now, in epoch milliseconds.
///
/// A clock the host reports as before the epoch, or a value too large for the
/// modelled timestamp, is logged rather than passed on silently: either would
/// order wrongly against the records already stored.
pub fn now_millis() -> i64 {
    match ::std::time::SystemTime::now().duration_since(::std::time::UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_millis()).unwrap_or_else(|_| {
            ::tracing::warn!("the host clock is past the representable range; clamping");
            i64::MAX
        }),
        Err(error) => {
            ::tracing::warn!(%error, "the host clock is before the epoch; using the epoch");
            0
        }
    }
}

/// Epoch milliseconds as the modelled timestamp type.
pub fn datetime_of_millis(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// A modelled timestamp as epoch milliseconds, so state comparisons are integer
/// comparisons.
pub fn millis_of_datetime(value: &::aws_smithy_types::DateTime) -> i64 {
    value.to_millis().unwrap_or_else(|error| {
        ::tracing::warn!(%error, "a request timestamp is outside the representable range");
        0
    })
}

// ---------------------------------------------------------------------------
// The tokenizer
// ---------------------------------------------------------------------------

/// The longest run of characters this emulator's tokenizer puts in one token.
///
/// The tokenizer is the emulator's own, and deliberately simple: a word is cut
/// into pieces of at most this many characters and every punctuation mark is a
/// token of its own. It is NOT any vendor's BPE vocabulary — no tokenizer
/// vocabulary ships with this emulator — but it is deterministic and it is the
/// SINGLE tokenizer every operation counts with, which is the property
/// `CountTokens` and `Converse` have to share.
pub const MAX_CHARS_PER_TOKEN: usize = 4;

/// The byte offset at which each token of `text` ends.
fn token_ends(text: &str) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut chars_in_piece = 0usize;
    for (offset, ch) in text.char_indices() {
        if ch.is_alphanumeric() {
            if chars_in_piece == MAX_CHARS_PER_TOKEN {
                ends.push(offset);
                chars_in_piece = 0;
            }
            chars_in_piece += 1;
        } else {
            if chars_in_piece > 0 {
                ends.push(offset);
                chars_in_piece = 0;
            }
            if !ch.is_whitespace() {
                ends.push(offset + ch.len_utf8());
            }
        }
    }
    if chars_in_piece > 0 {
        ends.push(text.len());
    }
    ends
}

/// How many tokens `text` is.
pub fn count_text_tokens(text: &str) -> u32 {
    u32::try_from(token_ends(text).len()).unwrap_or(u32::MAX)
}

/// A token count as the modelled `Integer`.
pub fn token_count_i32(tokens: u32) -> i32 {
    i32::try_from(tokens).unwrap_or(i32::MAX)
}

/// `text` cut to at most `max_tokens` tokens, on a token boundary.
pub fn truncate_to_tokens(text: &str, max_tokens: u32) -> String {
    if max_tokens == 0 {
        return String::new();
    }
    let ends = token_ends(text);
    match ends.get(max_tokens as usize - 1) {
        Some(_) if ends.len() > max_tokens as usize => text[..ends[max_tokens as usize - 1]]
            .trim_end()
            .to_string(),
        _ => text.to_string(),
    }
}

/// The tokens a `Converse`-shaped input is, counted over its text: the system
/// blocks, the message content blocks, and the names and descriptions of the
/// tools offered to the model. `CountTokens` and `Converse` both call this, so
/// the two agree for identical input.
pub fn converse_input_tokens(
    system: Option<&Vec<SystemContentBlock>>,
    messages: Option<&Vec<Message>>,
    tool_config: Option<&ToolConfiguration>,
) -> u32 {
    let mut total = 0u32;
    for block in system.map(Vec::as_slice).unwrap_or_default() {
        if let SystemContentBlock::Text(text) = block {
            total = total.saturating_add(count_text_tokens(text));
        }
    }
    for message in messages.map(Vec::as_slice).unwrap_or_default() {
        for block in &message.content {
            if let ContentBlock::Text(text) = block {
                total = total.saturating_add(count_text_tokens(text));
            }
        }
    }
    if let Some(config) = tool_config {
        for tool in &config.tools {
            if let Tool::toolSpec(spec) = tool {
                total = total.saturating_add(count_text_tokens(&spec.name));
                if let Some(description) = &spec.description {
                    total = total.saturating_add(count_text_tokens(description));
                }
            }
        }
    }
    total
}

/// The field names an `InvokeModel` request body carries prompt text under.
///
/// One rule covers every family in the registry, because the per-family request
/// formats in "Inference request parameters and response fields for foundation
/// models" (Amazon Bedrock User Guide) all bind their prompt text to one of
/// these names: Anthropic Claude's `messages[].content[].text` / `system` /
/// legacy `prompt`, Titan Text's `inputText`, Nova's `messages[].content[].text`
/// and `system[].text`, Llama's and Mistral's `prompt`, and Cohere's `prompt` /
/// `message`.
pub const PROMPT_TEXT_FIELDS: [&str; 6] = [
    "prompt",
    "inputText",
    "text",
    "message",
    "system",
    "content",
];

/// Every string the body binds under a [`PROMPT_TEXT_FIELDS`] name, joined by
/// newlines in key order. The walk stops at [`MAX_DOCUMENT_DEPTH`], so a deeply
/// nested body cannot drive unbounded recursion.
fn prompt_text_of_json(value: &serde_json::Value) -> String {
    fn walk(value: &serde_json::Value, field: Option<&str>, depth: usize, out: &mut Vec<String>) {
        if depth > MAX_DOCUMENT_DEPTH {
            return;
        }
        match value {
            serde_json::Value::String(text) => {
                if field.is_some_and(|name| PROMPT_TEXT_FIELDS.contains(&name)) {
                    out.push(text.clone());
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, field, depth + 1, out);
                }
            }
            serde_json::Value::Object(members) => {
                for (name, member) in members {
                    walk(member, Some(name.as_str()), depth + 1, out);
                }
            }
            _ => {}
        }
    }
    let mut parts = Vec::new();
    walk(value, None, 0, &mut parts);
    parts.join("\n")
}

/// The field a request body of each family carries its generation cap in.
///
/// Source: the per-family request formats in "Inference request parameters and
/// response fields for foundation models" (Amazon Bedrock User Guide) —
/// Anthropic's `max_tokens`, Titan's `textGenerationConfig.maxTokenCount`,
/// Nova's `inferenceConfig.max_new_tokens`, Llama's `max_gen_len`, and
/// Mistral's and Cohere's `max_tokens`.
pub fn body_max_tokens(family: ModelFamily, body: &serde_json::Value) -> Option<u32> {
    let pointer = match family {
        ModelFamily::AnthropicMessages => "/max_tokens",
        ModelFamily::TitanText => "/textGenerationConfig/maxTokenCount",
        ModelFamily::Nova => "/inferenceConfig/max_new_tokens",
        ModelFamily::Llama => "/max_gen_len",
        ModelFamily::Mistral | ModelFamily::CohereCommand => "/max_tokens",
    };
    body.pointer(pointer)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
}

/// The prompt text an `InvokeModel` body carries.
///
/// A JSON body contributes the strings it binds under a prompt-bearing field
/// name; a body that is not JSON, or that binds no such field, contributes its
/// own bytes as text, so no request is counted as zero tokens just because this
/// emulator does not know the field it used.
pub fn prompt_text_of_body(body: &[u8]) -> String {
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
        let text = prompt_text_of_json(&value);
        if !text.trim().is_empty() {
            return text;
        }
    }
    String::from_utf8_lossy(body).into_owned()
}

/// The tokens an `InvokeModel` body is. `CountTokens`'s `invokeModel` variant
/// and `InvokeModel` itself both count with this.
pub fn body_input_tokens(body: &[u8]) -> u32 {
    count_text_tokens(&prompt_text_of_body(body))
}

// ---------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------

/// A Smithy document as JSON. The walk stops at [`MAX_DOCUMENT_DEPTH`], past
/// which a branch renders as `null`, so a deeply nested caller document cannot
/// drive unbounded recursion.
///
/// Object members are emitted in SORTED key order. `Document::Object` is a
/// `HashMap`, whose iteration order differs between two instances holding the
/// same keys, and `serde_json::Map` preserves insertion order whenever anything
/// in the workspace enables `serde_json/preserve_order`. Without the sort, two
/// renderings of the same document can differ — which would make
/// [`async_invoke_fingerprint`] non-deterministic and turn an idempotent retry
/// into a `ConflictException`.
pub fn document_to_json(document: &::aws_smithy_types::Document) -> serde_json::Value {
    fn walk(document: &::aws_smithy_types::Document, depth: usize) -> serde_json::Value {
        use ::aws_smithy_types::{Document, Number};
        if depth > MAX_DOCUMENT_DEPTH {
            return serde_json::Value::Null;
        }
        match document {
            Document::Object(members) => {
                let mut pairs: Vec<(&String, &Document)> = members.iter().collect();
                pairs.sort_unstable_by_key(|(name, _)| *name);
                serde_json::Value::Object(
                    pairs
                        .into_iter()
                        .map(|(name, member)| (name.clone(), walk(member, depth + 1)))
                        .collect(),
                )
            }
            Document::Array(items) => {
                serde_json::Value::Array(items.iter().map(|item| walk(item, depth + 1)).collect())
            }
            Document::Number(Number::PosInt(value)) => serde_json::Value::from(*value),
            Document::Number(Number::NegInt(value)) => serde_json::Value::from(*value),
            Document::Number(Number::Float(value)) => serde_json::Number::from_f64(*value)
                .map_or(serde_json::Value::Null, serde_json::Value::Number),
            Document::String(value) => serde_json::Value::String(value.clone()),
            Document::Bool(value) => serde_json::Value::Bool(*value),
            Document::Null => serde_json::Value::Null,
        }
    }
    walk(document, 0)
}

// ---------------------------------------------------------------------------
// Completions
// ---------------------------------------------------------------------------

/// The text a synchronous inference call answers with.
///
/// No language model runs in this emulator, so this says so, and carries only
/// facts about the request that was actually served: how many messages it held
/// and how many tokens of input they were. It is never the request's own text
/// echoed back as if generated.
pub fn emulated_completion_text(model_id: &str, messages: u32, input_tokens: u32) -> String {
    format!(
        "No language model runs in this Amazon Bedrock Runtime emulator, so {model_id} generated \
         no text for this request. The request carried {messages} message(s) and {input_tokens} \
         input token(s)."
    )
}

/// The refusal message for an `InvokeModel` body that is not even a JSON object:
/// "You must provide the body in JSON format" is all the operation documents
/// about a body it cannot read.
pub const MALFORMED_BODY_MESSAGE: &str =
    "Malformed input request, please reformat your input and try again.";

/// The members a family's `InvokeModel` request body must bind.
///
/// One entry per required slot, as documented per family in "Inference request
/// parameters and response fields for foundation models" in the Amazon Bedrock
/// User Guide: the body must bind one of the slot's names (a family that takes
/// either a raw prompt or a chat transcript has both), and the FIRST name is the
/// one a refusal reports as the missing key. Slots are ordered with the member
/// carrying the prompt first, so a body that binds nothing at all is reported
/// against it.
fn required_body_slots(family: ModelFamily) -> &'static [&'static [&'static str]] {
    match family {
        // Anthropic Claude Messages API: messages, max_tokens and
        // anthropic_version are all documented Required.
        ModelFamily::AnthropicMessages => &[&["messages"], &["max_tokens"], &["anthropic_version"]],
        ModelFamily::TitanText => &[&["inputText"]],
        ModelFamily::Nova => &[&["messages"]],
        ModelFamily::Llama => &[&["prompt"]],
        // Mistral takes either a raw prompt or a chat transcript.
        ModelFamily::Mistral => &[&["prompt", "messages"]],
        // Cohere Command (text) takes `prompt`, Command R takes `message`.
        ModelFamily::CohereCommand => &[&["prompt", "message"]],
    }
}

/// The refusal `InvokeModel` owes for a body that does not satisfy `family`'s
/// documented request schema, or `None` when it does.
///
/// The body is model-specific, so this is the only place the request's own
/// shape is checked: a body that is not a JSON object cannot be read at all,
/// and one that omits a member the family requires is reported naming that
/// member, as a schema violation is.
pub fn invoke_body_violation(family: ModelFamily, body: &[u8]) -> Option<String> {
    let Ok(serde_json::Value::Object(members)) = serde_json::from_slice::<serde_json::Value>(body)
    else {
        return Some(MALFORMED_BODY_MESSAGE.to_string());
    };
    for slot in required_body_slots(family) {
        if slot.iter().any(|name| members.contains_key(*name)) {
            continue;
        }
        let name = slot.first().copied().unwrap_or_default();
        return Some(format!(
            "Malformed input request: #: required key [{name}] not found, please reformat your \
             input and try again."
        ));
    }
    None
}

/// The `InvokeModel` response body for `family`, in that family's own response
/// envelope.
///
/// Envelopes as documented per family in "Inference request parameters and
/// response fields for foundation models" in the Amazon Bedrock User Guide.
pub fn invoke_model_response_body(
    family: ModelFamily,
    model_id: &str,
    completion_id: &str,
    text: &str,
    input_tokens: u32,
    output_tokens: u32,
    stop_reason_is_max_tokens: bool,
) -> serde_json::Value {
    match family {
        ModelFamily::AnthropicMessages => serde_json::json!({
            "id": format!("msg_{completion_id}"),
            "type": "message",
            "role": "assistant",
            "model": model_id,
            "content": [{ "type": "text", "text": text }],
            "stop_reason": if stop_reason_is_max_tokens { "max_tokens" } else { "end_turn" },
            "stop_sequence": serde_json::Value::Null,
            "usage": { "input_tokens": input_tokens, "output_tokens": output_tokens },
        }),
        ModelFamily::TitanText => serde_json::json!({
            "inputTextTokenCount": input_tokens,
            "results": [{
                "tokenCount": output_tokens,
                "outputText": text,
                "completionReason": if stop_reason_is_max_tokens { "LENGTH" } else { "FINISH" },
            }],
        }),
        ModelFamily::Nova => serde_json::json!({
            "output": { "message": { "role": "assistant", "content": [{ "text": text }] } },
            "stopReason": if stop_reason_is_max_tokens { "max_tokens" } else { "end_turn" },
            "usage": {
                "inputTokens": input_tokens,
                "outputTokens": output_tokens,
                "totalTokens": input_tokens + output_tokens,
                // Nova reports its prompt-cache accounting on every
                // invocation. This emulator caches no prompt, so no token was
                // ever read from or written to a cache.
                "cacheReadInputTokenCount": 0,
                "cacheWriteInputTokenCount": 0,
            },
        }),
        ModelFamily::Llama => serde_json::json!({
            "generation": text,
            "prompt_token_count": input_tokens,
            "generation_token_count": output_tokens,
            "stop_reason": if stop_reason_is_max_tokens { "length" } else { "stop" },
        }),
        ModelFamily::Mistral => serde_json::json!({
            "outputs": [{
                "text": text,
                "stop_reason": if stop_reason_is_max_tokens { "length" } else { "stop" },
            }],
        }),
        ModelFamily::CohereCommand => serde_json::json!({
            "id": completion_id,
            "generations": [{
                "id": format!("gen_{completion_id}"),
                "text": text,
                "finish_reason": if stop_reason_is_max_tokens { "MAX_TOKENS" } else { "COMPLETE" },
            }],
        }),
    }
}
