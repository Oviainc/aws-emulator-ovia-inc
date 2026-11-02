// SPDX-License-Identifier: CC0-1.0

//! The WIRE FORM of the capability metadata a catalogue listing reports beside the members
//! `FoundationModelSummary` declares, and the SET of models it reports it for, end-to-end
//! over the same `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of catalogue rows rather than over one
//! recorded request:
//!
//! 1. **A capability is reported as the RECORD naming what is supported**, never as a bare
//!    boolean: `batchSupported` answers for the base model, for another region and for a
//!    customization of the model; `explicitPromptCaching`, `intelligentPromptRouting` and
//!    `converse.reasoningSupported` each name whether that capability is supported;
//!    `featuresSupported` names each Amazon Bedrock feature; and `converse` carries what a
//!    conversation may ask of the model. What one of them says about a model is more than
//!    one bit, so a client reads it by member name.
//! 2. **The capability metadata is published for a model that takes part in a conversation
//!    and for no other**: the summary of a model that answers with a vector or an image
//!    carries the modelled members and `guardrailsSupported`, and no family, invocation
//!    APIs, feature record, capability record or Converse record — the catalogue publishes
//!    it none, so the summary reports none rather than one it made up.
//! 3. **Every member of a record is read off the catalogue row**: the features the row
//!    publishes, the customizations it accepts, the modalities it takes, who publishes it.
//! 4. **The listing reports one summary per model the service offers**, so a client that
//!    enumerates the catalogue finds every published model id and can then address it; a
//!    read of one of them (`GetFoundationModel`) answers what the listing carries for it.
//!
//! The capability members are RECORDED behaviour: the on-disk model declares none of them
//! (`smithy_query(query="shape", shape="com.amazonaws.bedrock#FoundationModelSummary")`
//! lists modelArn/modelId/modelName/providerName/inputModalities/outputModalities/
//! responseStreamingSupported/customizationsSupported/inferenceTypesSupported/
//! modelLifecycle), so the expectations here come from the rules above, and the ONE
//! rendering of the whole projection lives in
//! `crate::support::expected_foundation_model_projection`.

use serde_json::Value;

use bedrock::state::{FoundationModel, FOUNDATION_MODELS};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::support::{
    emulator, expected_count_tokens_supported, expected_foundation_model_projection,
    expected_max_output_tokens, expected_reasoning_supported, label, Emulator, Reply, REGION,
};

/// The path `ListFoundationModels` is bound to, and the prefix `GetFoundationModel` reads
/// one model under (`smithy_query(query="operation",operation="ListFoundationModels")`).
const PATH: &str = "/foundation-models";

/// The members a summary carries ONLY for a model that takes part in a conversation.
const CONVERSATIONAL_ONLY_MEMBERS: &[&str] = &[
    "modelFamily",
    "inferenceAPIsSupported",
    "featuresSupported",
    "explicitPromptCaching",
    "intelligentPromptRouting",
    "latencyOptimizationSupported",
    "batchSupported",
    "converse",
];

/// The members of the record `featuresSupported` is.
const FEATURE_MEMBERS: &[&str] = &[
    "batchInference",
    "countTokens",
    "intelligentPromptRouting",
    "latencyOptimization",
    "promptCaching",
    "promptOptimization",
];

/// The members of the record `batchSupported` is.
const BATCH_MEMBERS: &[&str] = &[
    "baseModelSupported",
    "crossRegionSupported",
    "customModelSupported",
];

async fn list(emu: &Emulator) -> Reply {
    emu.call_in_region(REGION, "GET", PATH, None).await
}

async fn read(emu: &Emulator, model_id: &str) -> Reply {
    emu.call_in_region(REGION, "GET", &format!("{PATH}/{}", label(model_id)), None)
        .await
}

/// The summary the listing `reply` reported for `model_id`.
fn summary_of(case: &str, reply: &Reply, model_id: &str) -> Value {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: a listing must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .get("modelSummaries")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: modelSummaries must be a present array, got {:?}",
                reply.body
            )
        })
        .iter()
        .find(|row| row.get("modelId").and_then(Value::as_str) == Some(model_id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the listing must carry {model_id:?}"))
}

/// The catalogue row `model_id` names.
fn row(case: &str, model_id: &str) -> &'static FoundationModel {
    FOUNDATION_MODELS
        .iter()
        .find(|model| model.model_id == model_id)
        .unwrap_or_else(|| panic!("case {case:?}: the catalogue must publish {model_id:?}"))
}

// ---------------------------------------------------------------------------------------
// 1 and 3. A capability is a record, and every member of it is read off the row.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_capability_is_reported_as_the_record_that_names_what_is_supported() {
    // The rows differ along what the rule generalises over: which features the row
    // publishes (none, one, two, every one of them), whether the model accepts a
    // customization, whether it streams, which modalities it takes, who publishes it, and
    // whether it is available or deprecated.
    let cases = [
        "amazon.nova-pro-v1:0",
        "amazon.titan-text-lite-v1",
        "amazon.titan-text-premier-v1:0",
        "anthropic.claude-3-5-sonnet-20241022-v2:0",
        "anthropic.claude-3-haiku-20240307-v1:0",
        "cohere.command-text-v14",
        "meta.llama3-1-8b-instruct-v1:0",
        "moonshotai.kimi-k2.5",
        // Rows no check of the issue this suite was written for reads: a model published by
        // OpenAI, whose own endpoints Amazon Bedrock serves it on, and two models of
        // providers the catalogue gained beside it.
        "openai.gpt-6-astra",
        "qwen.qwen3-32b-v1:0",
        "zai.glm-5",
    ];

    let emu = emulator();
    let listing = list(&emu).await;
    for case in cases {
        let model = row(case, case);
        let summary = summary_of(case, &listing, case);
        let published = |feature: &str| model.features_supported.contains(&feature);

        // Each capability answers by MEMBER NAME, so none of them is a bare boolean.
        for member in ["explicitPromptCaching", "intelligentPromptRouting"] {
            let record = summary.get(member).unwrap_or(&Value::Null);
            assert!(
                record.is_object(),
                "case {case:?}: {member} must be the record naming what is supported, got \
                 {record:?}"
            );
        }

        let caching = summary["explicitPromptCaching"]["isSupported"].as_bool();
        assert_eq!(
            caching,
            Some(published("PROMPT_CACHING")),
            "case {case:?}: expected explicitPromptCaching.isSupported {:?}, got {caching:?}",
            published("PROMPT_CACHING")
        );
        let routing = summary["intelligentPromptRouting"]["isSupported"].as_bool();
        assert_eq!(
            routing,
            Some(published("INTELLIGENT_PROMPT_ROUTING")),
            "case {case:?}: expected intelligentPromptRouting.isSupported {:?}, got {routing:?}",
            published("INTELLIGENT_PROMPT_ROUTING")
        );

        // Batch inference answers for the base model, for another region, and for a
        // customization of the model — the last of which needs the model to accept a
        // customization at all.
        let batch = &summary["batchSupported"];
        for member in BATCH_MEMBERS {
            assert!(
                batch.get(member).and_then(Value::as_bool).is_some(),
                "case {case:?}: batchSupported must answer for {member}, got {batch:?}"
            );
        }
        let runs_batch = published("BATCH_INFERENCE");
        assert_eq!(
            batch["baseModelSupported"].as_bool(),
            Some(runs_batch),
            "case {case:?}: expected batchSupported.baseModelSupported {runs_batch:?}, got \
             {batch:?}"
        );
        let customizable = runs_batch && !model.customizations_supported.is_empty();
        assert_eq!(
            batch["customModelSupported"].as_bool(),
            Some(customizable),
            "case {case:?}: a customization of the model is batch-run only where the model \
             accepts one, so expected {customizable:?}, got {batch:?}"
        );

        // The feature record names every feature, each with the answer the row publishes.
        let features = &summary["featuresSupported"];
        for member in FEATURE_MEMBERS {
            assert!(
                features.get(member).and_then(Value::as_bool).is_some(),
                "case {case:?}: featuresSupported must answer for {member}, got {features:?}"
            );
        }
        assert_eq!(
            features["batchInference"].as_bool(),
            Some(runs_batch),
            "case {case:?}: expected featuresSupported.batchInference {runs_batch:?}, got \
             {features:?}"
        );
        // A token count is the model's own tokenizer's, so it is served for the one family
        // this emulator's inference plane carries a tokenizer for.
        let counts_tokens = expected_count_tokens_supported(model);
        assert_eq!(
            features["countTokens"].as_bool(),
            Some(counts_tokens),
            "case {case:?}: expected featuresSupported.countTokens {counts_tokens:?}, got \
             {features:?}"
        );

        // The Converse record carries what a conversation may ask of the model.
        let converse = &summary["converse"];
        assert_eq!(
            converse["streamingSupported"].as_bool(),
            Some(model.response_streaming_supported),
            "case {case:?}: expected converse.streamingSupported {:?}, got {converse:?}",
            model.response_streaming_supported
        );
        assert_eq!(
            converse["systemRoleSupported"].as_bool(),
            Some(true),
            "case {case:?}: a conversation carries a system role, got {converse:?}"
        );
        let reasoning = converse["reasoningSupported"]["isSupported"].as_bool();
        assert_eq!(
            reasoning,
            Some(expected_reasoning_supported(model)),
            "case {case:?}: expected converse.reasoningSupported.isSupported {:?}, got \
             {converse:?}",
            expected_reasoning_supported(model)
        );
        // The formats a turn may carry TO the model follow the modalities it takes.
        let images = converse["userImageTypesSupported"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        let takes_images = model.input_modalities.contains(&"IMAGE");
        assert_eq!(
            images > 0,
            takes_images,
            "case {case:?}: an image format is carried to a model that takes IMAGE ({takes_images:?}), \
             got {converse:?}"
        );
        assert_eq!(
            converse["userVideoTypesSupported"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: no modality the model declares is video, so no format is carried \
             to it, got {converse:?}"
        );
        // A ceiling is reported exactly where the catalogue publishes one for the model.
        let ceiling = expected_max_output_tokens(model);
        assert_eq!(
            converse.get("maxTokensMaximum").and_then(Value::as_i64),
            ceiling.map(i64::from),
            "case {case:?}: expected converse.maxTokensMaximum {ceiling:?}, got {converse:?}"
        );
        assert!(
            converse.get("maxTokensDefault").is_none(),
            "case {case:?}: a default for maxTokens is published nowhere, so it is not \
             reported, got {converse:?}"
        );

        // And the whole projection is the one the rule states for the row.
        let expected = expected_foundation_model_projection(model, REGION);
        assert_eq!(
            summary, expected,
            "case {case:?}: expected the projection {expected:?}, got {summary:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 2. The capability metadata is published for a conversational model and for no other.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_model_that_answers_no_turn_of_a_conversation_carries_no_capability_metadata() {
    // The rows differ along what the model answers with: an embedding vector (three of
    // them, published by two providers, one batch-run and one not), a relevance score, and
    // an image (a generation model and an editing model).
    let cases = [
        "amazon.titan-embed-image-v1",
        "amazon.titan-embed-text-v1",
        "cohere.embed-english-v3",
        "cohere.rerank-v3-5:0",
        "amazon.titan-image-generator-v1",
        "stability.stable-image-ultra-v1:1",
    ];

    let emu = emulator();
    let listing = list(&emu).await;
    for case in cases {
        let model = row(case, case);
        assert!(
            !model.output_modalities.contains(&"TEXT"),
            "case {case:?}: this row is in the table because it answers no turn of a \
             conversation, got {:?}",
            model.output_modalities
        );
        let summary = summary_of(case, &listing, case);
        for member in CONVERSATIONAL_ONLY_MEMBERS {
            assert!(
                summary.get(member).is_none(),
                "case {case:?}: the catalogue publishes no {member} for a model that answers \
                 no turn of a conversation, so the summary must not report one, got {:?}",
                summary.get(member)
            );
        }
        // What is decided BESIDE the invocation is still reported: a guardrail decides about
        // text, so it does not apply to this model.
        assert_eq!(
            summary.get("guardrailsSupported").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: expected guardrailsSupported false, got {:?}",
            summary.get("guardrailsSupported")
        );
        // The modelled members are all there.
        for member in [
            "modelArn",
            "modelId",
            "modelName",
            "providerName",
            "inputModalities",
            "outputModalities",
            "responseStreamingSupported",
            "customizationsSupported",
            "inferenceTypesSupported",
            "modelLifecycle",
        ] {
            assert!(
                summary.get(member).is_some(),
                "case {case:?}: {member} is modelled, so every summary carries it, got \
                 {summary:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 4. The listing reports one summary per offered model, and a read agrees with it.
// ---------------------------------------------------------------------------------------

/// Model ids the service's catalogue publishes, spanning the providers it offers.
///
/// Written out here rather than read off [`FOUNDATION_MODELS`] so the suite states WHICH
/// models a client must find, and taken from the ids the published catalogue carries ("Amazon
/// Bedrock base model IDs (on-demand throughput)" and "Supported foundation models in Amazon
/// Bedrock" in the Amazon Bedrock User Guide, which is what `state::FOUNDATION_MODELS`
/// mirrors). None of these is an id the issue this suite was written for cites.
const PUBLISHED_MODEL_IDS: &[&str] = &[
    "amazon.nova-2-lite-v1:0",
    "amazon.rerank-v1:0",
    "amazon.titan-embed-g1-text-02",
    "anthropic.claude-haiku-4-5-20251001-v1:0",
    "anthropic.claude-opus-4-6-v1",
    "anthropic.claude-sonnet-4-5-20250929-v1:0",
    "cohere.embed-v4:0",
    "deepseek.r1-v1:0",
    "google.gemma-3-27b-it",
    "luma.ray-v2:0",
    "meta.llama4-scout-17b-instruct-v1:0",
    "minimax.minimax-m2",
    "mistral.pixtral-large-2502-v1:0",
    "moonshot.kimi-k2-thinking",
    "nvidia.nemotron-nano-9b-v2",
    "openai.gpt-oss-120b-1:0",
    "qwen.qwen3-vl-235b-a22b",
    "stability.stable-image-core-v1:1",
    "twelvelabs.pegasus-1-2-v1:0",
    "writer.palmyra-x5-v1:0",
    "xai.grok-4.7",
    "zai.glm-4.7-flash",
];

#[tokio::test]
async fn a_client_finds_every_published_model_in_the_listing_and_reads_it_back() {
    let emu = emulator();
    let listing = list(&emu).await;
    let listed: Vec<String> = listing
        .get("modelSummaries")
        .as_array()
        .unwrap_or_else(|| panic!("the listing must carry modelSummaries, got {:?}", listing.body))
        .iter()
        .filter_map(|row| row.get("modelId").and_then(Value::as_str).map(str::to_string))
        .collect();

    for case in PUBLISHED_MODEL_IDS {
        assert!(
            listed.iter().any(|model_id| model_id == case),
            "case {case:?}: a client enumerating the catalogue must find this model, got a \
             listing of {} models without it",
            listed.len()
        );
        // And the read of that one model answers what the listing carries for it, so a
        // client that discovers a model and then reads it is not told two different things.
        let summary = summary_of(case, &listing, case);
        let reply = read(&emu, case).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a read of a published model must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        let details = reply.get("modelDetails");
        assert_eq!(
            details, summary,
            "case {case:?}: expected the read to report the listed projection {summary:?}, got \
             {details:?}"
        );
    }

    // The listing reports one summary per offered model and no more, whatever it reports
    // for each of them.
    assert_eq!(
        listed.len(),
        FOUNDATION_MODELS.len(),
        "case \"the whole catalogue\": expected one summary per offered model ({}), got {}",
        FOUNDATION_MODELS.len(),
        listed.len()
    );
}
