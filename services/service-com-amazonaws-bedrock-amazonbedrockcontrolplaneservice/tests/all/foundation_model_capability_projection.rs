// SPDX-License-Identifier: CC0-1.0

//! The CAPABILITY projection of the published foundation model catalog, end-to-end over the
//! same `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of models rather than over one recorded
//! request:
//!
//! 1. **A listing reports the whole projection of every catalog row**: beside the ten
//!    members `FoundationModelSummary` declares
//!    (`smithy_query(query="shape",shape="FoundationModelSummary")`), each summary carries
//!    the model's family and what may be done with it — `inferenceAPIsSupported`,
//!    `featuresSupported`, `guardrailsSupported`, `explicitPromptCaching`,
//!    `intelligentPromptRouting`, `latencyOptimizationSupported`, `batchSupported` and
//!    `converse` — every one of them read off that row.
//! 2. **A read reports the same projection as the listing**: `GetFoundationModel` on a
//!    model answers member for member what the listing carries for it, so a client that
//!    discovers a model and then reads it cannot be told two different things about it.
//! 3. **A feature holds of exactly the models that publish it**: `featuresSupported` is the
//!    set of Amazon Bedrock features a row publishes support for, and the four feature
//!    members report membership in that set one feature at a time — so a model no feature's
//!    supported-models table lists, which is where a freshly offered model starts, supports
//!    none of them whatever its modalities, streaming or inference types are.
//! 4. **What a model itself establishes is decided from the row**: `invokeModel` reaches
//!    every offered model; `converse` carries a CONVERSATION, so it reaches a model that
//!    produces TEXT and streams through it when the model streams; the OpenAI-compatible
//!    endpoints reach the models that provider publishes; and a guardrail applies to a
//!    model that produces TEXT, because a guardrail decides about text and is applied
//!    beside the invocation.
//! 5. **The account-level access default travels with the listing**: a response reports
//!    once, beside the summaries, whether this account reaches every model the catalog
//!    offers without requesting access per model — whatever filters the request carried and
//!    whatever region it addressed.
//!
//! Rules 3 and 4 are tabled over SYNTHETIC rows, because they span classes the published
//! catalog does not hold a row of (a model offered only with reserved throughput, a model
//! published by OpenAI), and because a table of rows that differ in ONE attribute is what
//! shows a member is decided by that attribute and not by another.
//!
//! The capability members and the account-level flag are RECORDED behaviour: the on-disk
//! model declares neither (`query="shape"` on `FoundationModelSummary` and on
//! `ListFoundationModelsResponse` list ten members and one), so the expectations here come
//! from the rules above rather than from the model, and the ONE rendering of them lives in
//! `crate::support::expected_foundation_model_projection`.

use serde_json::{json, Value};

use bedrock::state::{FoundationModel, FOUNDATION_MODELS};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::support::{
    emulator, expected_foundation_model_projection, label, Emulator, Reply, REGION,
};

/// The path `ListFoundationModels` is bound to, and the prefix `GetFoundationModel` reads
/// one model under
/// (`smithy_query(query="operation",operation="ListFoundationModels")`).
const PATH: &str = "/foundation-models";

async fn list(emu: &Emulator, region: &str, query: &str) -> Reply {
    let path = if query.is_empty() {
        PATH.to_string()
    } else {
        format!("{PATH}?{query}")
    };
    emu.call_in_region(region, "GET", &path, None).await
}

async fn read(emu: &Emulator, region: &str, model_id: &str) -> Reply {
    emu.call_in_region(region, "GET", &format!("{PATH}/{}", label(model_id)), None)
        .await
}

/// The summaries a listing reported, in the order it reported them.
fn summaries(case: &str, reply: &Reply) -> Vec<Value> {
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
        .clone()
}

/// The one summary a listing reported for `model_id`.
fn summary_of(case: &str, reply: &Reply, model_id: &str) -> Value {
    summaries(case, reply)
        .into_iter()
        .find(|row| row.get("modelId").and_then(Value::as_str) == Some(model_id))
        .unwrap_or_else(|| panic!("case {case:?}: the listing must carry {model_id:?}"))
}

// ---------------------------------------------------------------------------------------
// 1. A listing reports the whole projection of every catalog row.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn every_catalogued_model_reports_its_whole_capability_projection() {
    let emu = emulator();
    let reply = list(&emu, REGION, "").await;
    for model in FOUNDATION_MODELS {
        let case = model.model_id;
        let row = summary_of(case, &reply, model.model_id);
        let expected = expected_foundation_model_projection(model, REGION);
        assert_eq!(
            row, expected,
            "case {case:?}: expected the projection {expected:?}, got {row:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 2. A read reports the same projection as the listing.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_read_of_a_model_reports_the_projection_the_listing_carries_for_it() {
    // The rows differ along the region the request signs for: the capability projection is
    // published data, so it must not vary with the region, while the ARN inside it does.
    let regions = ["us-west-2", "eu-central-1"];

    let emu = emulator();
    for region in regions {
        let listing = list(&emu, region, "").await;
        for model in FOUNDATION_MODELS {
            let case = format!("{region} {}", model.model_id);
            let listed = summary_of(&case, &listing, model.model_id);
            let reply = read(&emu, region, model.model_id).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a read must answer 200, got {} {:?}",
                reply.status, reply.body
            );
            let details = reply.get("modelDetails");
            assert_eq!(
                details, listed,
                "case {case:?}: expected the read to report the listed projection {listed:?}, \
                 got {details:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 3 and 4. A feature holds of exactly the models that publish it; what the model itself
// establishes is decided from the row.
// ---------------------------------------------------------------------------------------

/// A row of the table: a model built to sit in ONE class of the rules, and the capability
/// members that class must produce.
struct Case {
    name: &'static str,
    model: FoundationModel,
    inference_apis: &'static [&'static str],
    guardrails: bool,
    caching: bool,
    routing: bool,
    latency: bool,
    batch: bool,
    converse: bool,
    converse_streaming: bool,
}

/// A catalog row, for the one class a case puts it in.
const fn row(
    model_id: &'static str,
    provider_name: &'static str,
    input_modalities: &'static [&'static str],
    output_modalities: &'static [&'static str],
    response_streaming_supported: bool,
    inference_types_supported: &'static [&'static str],
    features_supported: &'static [&'static str],
) -> FoundationModel {
    FoundationModel {
        model_id,
        model_name: "Table Row",
        provider_name,
        model_family: "Table",
        input_modalities,
        output_modalities,
        response_streaming_supported,
        customizations_supported: &[],
        inference_types_supported,
        features_supported,
        lifecycle_status: "ACTIVE",
        start_of_life_time: "2024-01-01T00:00:00Z",
        legacy_window: None,
    }
}

/// Every feature a row can publish, in the order a summary reports them.
const ALL_FEATURES: &[&str] = &[
    "BATCH_INFERENCE",
    "INTELLIGENT_PROMPT_ROUTING",
    "LATENCY_OPTIMIZATION",
    "PROMPT_CACHING",
];

/// The APIs a model that produces TEXT and is published by anybody but OpenAI is reachable
/// through: Amazon Bedrock's own conversation API, and `invokeModel`, which reaches every
/// offered model.
const CONVERSE_AND_INVOKE: &[&str] = &["converse", "invokeModel"];

#[tokio::test]
async fn a_capability_holds_of_exactly_the_models_whose_attributes_establish_it() {
    // The rows differ along what the rules generalise over: which features the row
    // publishes (all four, none, one, two), the output modality (TEXT, IMAGE, EMBEDDING),
    // whether the model streams, whether it is offered ON_DEMAND or only with reserved
    // throughput, and who publishes it.
    let cases = [
        Case {
            name: "a streaming text model every feature's table lists",
            model: row(
                "table.text-every-feature",
                "Table AI",
                &["TEXT"],
                &["TEXT"],
                true,
                &["ON_DEMAND"],
                ALL_FEATURES,
            ),
            inference_apis: CONVERSE_AND_INVOKE,
            guardrails: true,
            caching: true,
            routing: true,
            latency: true,
            batch: true,
            converse: true,
            converse_streaming: true,
        },
        Case {
            name: "the same model, freshly offered, listed by no feature's table",
            model: row(
                "table.text-no-feature",
                "Table AI",
                &["TEXT"],
                &["TEXT"],
                true,
                &["ON_DEMAND"],
                &[],
            ),
            // Converse and guardrails are decided by what the model does, so they hold of
            // this row exactly as they do of the one above; the four features do not.
            inference_apis: CONVERSE_AND_INVOKE,
            guardrails: true,
            caching: false,
            routing: false,
            latency: false,
            batch: false,
            converse: true,
            converse_streaming: true,
        },
        Case {
            name: "a text model only batch inference lists",
            model: row(
                "table.text-batch",
                "Table AI",
                &["TEXT", "IMAGE"],
                &["TEXT"],
                true,
                &["ON_DEMAND", "PROVISIONED"],
                &["BATCH_INFERENCE"],
            ),
            inference_apis: CONVERSE_AND_INVOKE,
            guardrails: true,
            caching: false,
            routing: false,
            latency: false,
            batch: true,
            converse: true,
            converse_streaming: true,
        },
        Case {
            name: "a text model that answers in one piece, routed to and cached for",
            model: row(
                "table.text-unary",
                "Table AI",
                &["TEXT"],
                &["TEXT"],
                false,
                &["ON_DEMAND"],
                &["INTELLIGENT_PROMPT_ROUTING", "PROMPT_CACHING"],
            ),
            inference_apis: CONVERSE_AND_INVOKE,
            guardrails: true,
            caching: true,
            routing: true,
            latency: false,
            batch: false,
            converse: true,
            // Nothing is streamed, so Converse streaming is not reported for it.
            converse_streaming: false,
        },
        Case {
            name: "a text model offered only with reserved throughput",
            model: row(
                "table.text-provisioned",
                "Table AI",
                &["TEXT"],
                &["TEXT"],
                true,
                &["PROVISIONED"],
                &["BATCH_INFERENCE"],
            ),
            inference_apis: CONVERSE_AND_INVOKE,
            guardrails: true,
            caching: false,
            routing: false,
            latency: false,
            batch: true,
            converse: true,
            converse_streaming: true,
        },
        Case {
            name: "an embedding model batch inference lists",
            model: row(
                "table.embedding",
                "Table AI",
                &["TEXT"],
                &["EMBEDDING"],
                false,
                &["ON_DEMAND", "PROVISIONED"],
                &["BATCH_INFERENCE"],
            ),
            // A vector is no turn of a conversation, so Converse does not reach it.
            inference_apis: &["invokeModel"],
            guardrails: false,
            caching: false,
            routing: false,
            latency: false,
            batch: true,
            converse: false,
            converse_streaming: false,
        },
        Case {
            name: "an image model no feature's table lists",
            model: row(
                "table.image",
                "Table AI",
                &["TEXT", "IMAGE"],
                &["IMAGE"],
                false,
                &["ON_DEMAND"],
                &[],
            ),
            inference_apis: &["invokeModel"],
            guardrails: false,
            caching: false,
            routing: false,
            latency: false,
            batch: false,
            converse: false,
            converse_streaming: false,
        },
        Case {
            name: "a text model its provider also serves on its own OpenAI-compatible API",
            model: row(
                "openai.table-model",
                "OpenAI",
                &["TEXT", "IMAGE"],
                &["TEXT"],
                true,
                &["ON_DEMAND"],
                &["BATCH_INFERENCE"],
            ),
            // Who publishes a model decides which of its provider's own APIs Amazon Bedrock
            // serves it on, and nothing else about it: the four features and Converse hold
            // of this row exactly as they do of a row published by anybody else.
            inference_apis: &[
                "converse",
                "invokeModel",
                "openAiChatCompletions",
                "openAiResponses",
            ],
            guardrails: true,
            caching: false,
            routing: false,
            latency: false,
            batch: true,
            converse: true,
            converse_streaming: true,
        },
    ];

    for case in &cases {
        let name = case.name;
        let model = &case.model;

        let apis = bedrock::state::model_inference_apis_supported(model);
        assert_eq!(
            apis, case.inference_apis,
            "case {name:?}: expected inferenceAPIsSupported {:?}, got {apis:?}",
            case.inference_apis
        );
        // `featuresSupported` names each published feature, and the capability members
        // below report one feature at a time, so the record and those members agree.
        let features = bedrock::state::model_features_supported(model);
        assert_eq!(
            features.batch_inference,
            Some(case.batch),
            "case {name:?}: expected featuresSupported.batchInference {:?}, got {features:?}",
            case.batch
        );
        assert_eq!(
            features.intelligent_prompt_routing,
            Some(case.routing),
            "case {name:?}: expected featuresSupported.intelligentPromptRouting {:?}, got \
             {features:?}",
            case.routing
        );
        assert_eq!(
            features.latency_optimization,
            Some(case.latency),
            "case {name:?}: expected featuresSupported.latencyOptimization {:?}, got {features:?}",
            case.latency
        );
        assert_eq!(
            features.prompt_caching,
            Some(case.caching),
            "case {name:?}: expected featuresSupported.promptCaching {:?}, got {features:?}",
            case.caching
        );
        let guardrails = bedrock::state::model_guardrails_supported(model);
        assert_eq!(
            guardrails, case.guardrails,
            "case {name:?}: expected guardrailsSupported {}, got {guardrails}",
            case.guardrails
        );
        let caching = bedrock::state::model_explicit_prompt_caching(model);
        assert_eq!(
            caching, case.caching,
            "case {name:?}: expected explicitPromptCaching {}, got {caching}",
            case.caching
        );
        let routing = bedrock::state::model_intelligent_prompt_routing(model);
        assert_eq!(
            routing, case.routing,
            "case {name:?}: expected intelligentPromptRouting {}, got {routing}",
            case.routing
        );
        let latency = bedrock::state::model_latency_optimization_supported(model);
        assert_eq!(
            latency, case.latency,
            "case {name:?}: expected latencyOptimizationSupported {}, got {latency}",
            case.latency
        );
        let batch = bedrock::state::model_batch_support(model);
        assert_eq!(
            batch.base_model_supported,
            Some(case.batch),
            "case {name:?}: expected batchSupported.baseModelSupported {:?}, got {batch:?}",
            case.batch
        );
        let converse = bedrock::state::model_converse_support(model);
        assert_eq!(
            converse.is_some(),
            case.converse,
            "case {name:?}: expected a converse record to be reported: {:?}, got {converse:?}",
            case.converse
        );
        let streaming = converse
            .as_ref()
            .and_then(|converse| converse.streaming_supported);
        assert_eq!(
            streaming,
            case.converse.then_some(case.converse_streaming),
            "case {name:?}: expected converse.streamingSupported {:?}, got {converse:?}",
            case.converse_streaming
        );
    }

    // Every feature name a row may publish is one of the four, so a row cannot put a
    // feature on the wire that no member of the summary reports.
    assert_eq!(
        bedrock::state::PUBLISHED_FEATURE_VALUES,
        ALL_FEATURES,
        "case \"the published feature set\": expected {ALL_FEATURES:?}, got {:?}",
        bedrock::state::PUBLISHED_FEATURE_VALUES
    );
    for model in FOUNDATION_MODELS {
        for feature in model.features_supported {
            assert!(
                ALL_FEATURES.contains(feature),
                "case {:?}: {feature:?} is no published feature, so no member of the summary \
                 reports it",
                model.model_id
            );
        }
    }

    // The classes the table covers are classes a listing has to serve too, so the published
    // rows the emulator does hold are checked against the same rule through the wire —
    // including rows no check of this issue reads: an image model, two embedding models, a
    // deprecated text model and text models offered on demand alone.
    let emu = emulator();
    let reply = list(&emu, REGION, "").await;
    let spanned = [
        "amazon.nova-canvas-v1:0",
        "amazon.nova-pro-v1:0",
        "amazon.titan-embed-text-v2:0",
        "amazon.titan-embed-image-v1",
        "amazon.titan-text-lite-v1",
        "ai21.jamba-1-5-mini-v1:0",
        "mistral.mixtral-8x7b-instruct-v0:1",
        "moonshotai.kimi-k2.5",
    ];
    for model_id in spanned {
        let model = FOUNDATION_MODELS
            .iter()
            .find(|model| model.model_id == model_id)
            .unwrap_or_else(|| panic!("case {model_id:?}: the catalog must carry this row"));
        let row = summary_of(model_id, &reply, model_id);
        let expected = expected_foundation_model_projection(model, REGION);
        assert_eq!(
            row, expected,
            "case {model_id:?}: expected the projection {expected:?}, got {row:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 5. The account-level access default travels with the listing.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_listing_reports_the_account_level_access_default_whatever_it_was_asked_for() {
    // The rows differ along what the listing was asked for: the whole catalog, each kind of
    // filter, a conjunction of two, a filter that admits nothing, and another region.
    let cases = [
        ("the whole catalog", REGION, ""),
        ("filtered by provider", REGION, "byProvider=Amazon"),
        (
            "filtered by output modality",
            REGION,
            "byOutputModality=EMBEDDING",
        ),
        (
            "filtered by inference type",
            REGION,
            "byInferenceType=PROVISIONED",
        ),
        (
            "filtered by customization type",
            REGION,
            "byCustomizationType=FINE_TUNING",
        ),
        (
            "two filters at once",
            REGION,
            "byProvider=Amazon&byOutputModality=TEXT",
        ),
        ("a filter that admits nothing", REGION, "byProvider=Nonesuch"),
        ("another region", "eu-central-1", ""),
    ];

    let emu = emulator();
    // The recorded value, stated here rather than read off the constant the handler reads,
    // so the expectation does not move if the published default is changed by mistake.
    let recorded = json!(true);
    assert_eq!(
        json!(bedrock::state::ACCESS_TO_ALL_MODELS_BY_DEFAULT),
        recorded,
        "case \"the published default\": an account reaches every offered model by default, \
         so the published default must be {recorded}, got {:?}",
        bedrock::state::ACCESS_TO_ALL_MODELS_BY_DEFAULT
    );
    for (name, region, query) in cases {
        let reply = list(&emu, region, query).await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: a listing must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        let flag = reply.get("enableAccessToAllModelsByDefault");
        assert_eq!(
            flag, recorded,
            "case {name:?}: expected enableAccessToAllModelsByDefault {recorded}, got {flag:?}"
        );
    }
}

#[tokio::test]
async fn two_listings_report_the_same_projection_and_the_same_access_default() {
    let emu = emulator();
    let first = list(&emu, REGION, "").await;
    let second = list(&emu, REGION, "").await;
    assert_eq!(
        first.body, second.body,
        "case \"repeated listing\": a listing is a pure read, so two of them must answer \
         alike, got {:?} then {:?}",
        first.body, second.body
    );
}
