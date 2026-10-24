// SPDX-License-Identifier: CC0-1.0

//! `ListFoundationModels` end-to-end, over the same `MultiServiceRouter` the emulator
//! boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **The listing IS the published catalog**: one `FoundationModelSummary` per row of
//!    the catalog the store is seeded with, in `modelId` order, every member reported out
//!    of that row — so the response is read off state rather than written per request.
//! 2. **Each `by*` member narrows the listing on one published attribute, and filters
//!    combine by conjunction**: the models a filtered listing reports are exactly the
//!    catalog rows whose own attribute satisfies every filter the request carried. The
//!    rows span each filterable member, every value of each of them, both spellings
//!    `byProvider` accepts, conjunctions of two and three filters, and filters that admit
//!    nothing.
//! 3. **A model's ARN is the `foundation-model` ARN for the region the request
//!    addresses**, with the EMPTY account field `FoundationModelArn` declares
//!    (`smithy_query(query="shape",shape="FoundationModelSummary")`), and the catalog
//!    itself does not vary by region.
//! 4. **A value no filter member admits is refused with 400 `ValidationException`** by the
//!    constraints the model declares for it, never listed around.
//! 5. **Listing is a pure read**: two identical listings answer identically, and a listing
//!    does not disturb the guardrail store beside it.
//! 6. **Every catalog row is modelled data**: each value is one the matching Smithy enum
//!    carries and each name satisfies its member's `@length`, and the catalog spans every
//!    value a caller can filter on, which is what makes rule 2's table meaningful.
//!
//! Expectations come from the model
//! (`smithy_query(query="operation",operation="ListFoundationModels")` and
//! `query="shape"` on `FoundationModelSummary`, `ModelModality`, `ModelCustomization`,
//! `InferenceType`, `FoundationModelLifecycleStatus`, `BrandedName`, `Provider`), and the
//! requests go through `crate::support` — the ONE harness every suite in this crate drives
//! the emulator with.

use serde_json::{json, Value};

use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::ops::types::{
    FOUNDATION_MODEL_LIFECYCLE_STATUS_VALUES, INFERENCE_TYPE_VALUES, MODEL_CUSTOMIZATION_VALUES,
    MODEL_MODALITY_VALUES,
};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::{
    FoundationModel, FOUNDATION_MODELS,
};

use crate::support::{emulator, error_code, Emulator, Reply, REGION};

/// The one path `ListFoundationModels` is bound to
/// (`smithy_query(query="operation",operation="ListFoundationModels")`: `GET
/// /foundation-models`).
const PATH: &str = "/foundation-models";

/// `BrandedName`'s declared length span, which `modelName` and `providerName` both target.
const BRANDED_NAME_LENGTH: std::ops::RangeInclusive<usize> = 1..=20;

/// `BedrockModelId`'s declared maximum length, which `modelId` targets.
const MODEL_ID_MAX_LENGTH: usize = 140;

async fn list(emu: &Emulator, query: &str) -> Reply {
    list_in_region(emu, REGION, query).await
}

async fn list_in_region(emu: &Emulator, region: &str, query: &str) -> Reply {
    let path = if query.is_empty() {
        PATH.to_string()
    } else {
        format!("{PATH}?{query}")
    };
    emu.call_in_region(region, "GET", &path, None).await
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

/// The `modelId`s a listing reported, in the order it reported them.
fn listed_ids(case: &str, reply: &Reply) -> Vec<String> {
    summaries(case, reply)
        .iter()
        .map(|row| {
            row.get("modelId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a summary must carry modelId: {row:?}"))
                .to_string()
        })
        .collect()
}

/// The catalog rows a predicate admits, as their `modelId`s in catalog order.
fn expected_ids(admits: impl Fn(&FoundationModel) -> bool) -> Vec<String> {
    let mut ids: Vec<String> = FOUNDATION_MODELS
        .iter()
        .filter(|model| admits(model))
        .map(|model| model.model_id.to_string())
        .collect();
    ids.sort();
    ids
}

/// Whether `values` carries `wanted`.
fn carries(values: &[&str], wanted: &str) -> bool {
    values.contains(&wanted)
}

// ---------------------------------------------------------------------------------------
// 1 and 3. The listing is the catalog, with every member read off its row.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_unfiltered_listing_reports_every_catalog_row_in_model_id_order() {
    let emu = emulator();
    let reply = list(&emu, "").await;
    let rows = summaries("unfiltered", &reply);

    assert_eq!(
        rows.len(),
        FOUNDATION_MODELS.len(),
        "case \"unfiltered\": expected one summary per catalog row ({}), got {} ({:?})",
        FOUNDATION_MODELS.len(),
        rows.len(),
        listed_ids("unfiltered", &reply)
    );

    let actual = listed_ids("unfiltered", &reply);
    let mut sorted = actual.clone();
    sorted.sort();
    assert_eq!(
        actual, sorted,
        "case \"unfiltered\": the listing must be in modelId order, got {actual:?}"
    );
    assert_eq!(
        actual,
        expected_ids(|_| true),
        "case \"unfiltered\": the listed ids must be the catalog's"
    );

    for model in FOUNDATION_MODELS {
        let case = model.model_id;
        let row = rows
            .iter()
            .find(|row| row.get("modelId").and_then(Value::as_str) == Some(model.model_id))
            .unwrap_or_else(|| panic!("case {case:?}: the listing must carry this catalog row"));

        let expected = json!({
            "modelArn": format!("arn:aws:bedrock:{REGION}::foundation-model/{}", model.model_id),
            "modelId": model.model_id,
            "modelName": model.model_name,
            "providerName": model.provider_name,
            "inputModalities": model.input_modalities,
            "outputModalities": model.output_modalities,
            "responseStreamingSupported": model.response_streaming_supported,
            "customizationsSupported": model.customizations_supported,
            "inferenceTypesSupported": model.inference_types_supported,
            // Only `status`: no page publishes a launch, legacy or end-of-life instant per
            // model, so a summary reports none of the four.
            "modelLifecycle": { "status": model.lifecycle_status },
        });
        assert_eq!(
            row, &expected,
            "case {case:?}: expected the summary {expected:?}, got {row:?}"
        );
    }
}

#[tokio::test]
async fn a_model_arn_carries_the_region_the_request_addresses_and_no_account() {
    // The rows differ along the region the request signs for: the suite's own, another
    // partition-mate, and one in a third geography.
    let regions = ["us-west-2", "eu-central-1", "ap-southeast-2"];

    let emu = emulator();
    for region in regions {
        let reply = list_in_region(&emu, region, "").await;
        let rows = summaries(region, &reply);
        assert_eq!(
            listed_ids(region, &reply),
            expected_ids(|_| true),
            "case {region:?}: the catalog must not vary by region"
        );
        for row in &rows {
            let arn = row
                .get("modelArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {region:?}: a summary must carry modelArn: {row:?}"));
            let model_id = row.get("modelId").and_then(Value::as_str).unwrap_or_default();
            let expected = format!("arn:aws:bedrock:{region}::foundation-model/{model_id}");
            assert_eq!(
                arn, expected,
                "case {region:?}: expected the ARN {expected:?}, got {arn:?}"
            );
            // The account field of a `foundation-model` ARN is empty: the model is
            // published by AWS rather than owned by the account reading it.
            let account = arn.split(':').nth(4).unwrap_or("missing");
            assert_eq!(
                account, "",
                "case {region:?}: a foundation-model ARN carries no account, got {account:?} in \
                 {arn:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 2. Each filter narrows on one attribute, and filters combine by conjunction.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn every_filter_reports_exactly_the_catalog_rows_it_admits() {
    /// One filtered listing: the query it sends, and the catalog rows it must report.
    struct Row {
        case: &'static str,
        query: &'static str,
        admits: Box<dyn Fn(&FoundationModel) -> bool>,
    }

    fn row(
        case: &'static str,
        query: &'static str,
        admits: impl Fn(&FoundationModel) -> bool + 'static,
    ) -> Row {
        Row {
            case,
            query,
            admits: Box::new(admits),
        }
    }

    // The rows span each filterable member, every value of each of them, both spellings
    // `byProvider` takes (the branded name and the lowercase provider segment of the
    // model id), a provider whose branded name carries a space, conjunctions of two and
    // three filters, and filters that admit nothing.
    let rows: Vec<Row> = vec![
        row("no filter at all", "", |_| true),
        row("byProvider as the branded name", "byProvider=Amazon", |m| {
            m.provider_name == "Amazon"
        }),
        row(
            "byProvider lowercased, as the CLI's own examples pass it",
            "byProvider=anthropic",
            |m| m.provider_name == "Anthropic",
        ),
        row(
            "byProvider as the model id's provider segment",
            "byProvider=mistral",
            |m| m.provider_name == "Mistral AI",
        ),
        row(
            "byProvider as a branded name carrying a space",
            "byProvider=Mistral%20AI",
            |m| m.provider_name == "Mistral AI",
        ),
        row("byProvider naming a provider of one model", "byProvider=stability", |m| {
            m.provider_name == "Stability AI"
        }),
        row("byProvider naming nobody", "byProvider=Nonesuch", |_| false),
        row("byOutputModality=TEXT", "byOutputModality=TEXT", |m| {
            carries(m.output_modalities, "TEXT")
        }),
        row("byOutputModality=IMAGE", "byOutputModality=IMAGE", |m| {
            carries(m.output_modalities, "IMAGE")
        }),
        row("byOutputModality=EMBEDDING", "byOutputModality=EMBEDDING", |m| {
            carries(m.output_modalities, "EMBEDDING")
        }),
        row(
            "byCustomizationType=FINE_TUNING",
            "byCustomizationType=FINE_TUNING",
            |m| carries(m.customizations_supported, "FINE_TUNING"),
        ),
        row(
            "byCustomizationType=CONTINUED_PRE_TRAINING",
            "byCustomizationType=CONTINUED_PRE_TRAINING",
            |m| carries(m.customizations_supported, "CONTINUED_PRE_TRAINING"),
        ),
        row(
            "byCustomizationType=DISTILLATION",
            "byCustomizationType=DISTILLATION",
            |m| carries(m.customizations_supported, "DISTILLATION"),
        ),
        row("byInferenceType=ON_DEMAND", "byInferenceType=ON_DEMAND", |m| {
            carries(m.inference_types_supported, "ON_DEMAND")
        }),
        row("byInferenceType=PROVISIONED", "byInferenceType=PROVISIONED", |m| {
            carries(m.inference_types_supported, "PROVISIONED")
        }),
        row(
            "two filters conjoined",
            "byProvider=Amazon&byOutputModality=IMAGE",
            |m| m.provider_name == "Amazon" && carries(m.output_modalities, "IMAGE"),
        ),
        row(
            "three filters conjoined",
            "byOutputModality=TEXT&byCustomizationType=FINE_TUNING&byInferenceType=PROVISIONED",
            |m| {
                carries(m.output_modalities, "TEXT")
                    && carries(m.customizations_supported, "FINE_TUNING")
                    && carries(m.inference_types_supported, "PROVISIONED")
            },
        ),
        row(
            "a conjunction no model satisfies",
            "byProvider=Cohere&byOutputModality=IMAGE",
            |m| m.provider_name == "Cohere" && carries(m.output_modalities, "IMAGE"),
        ),
    ];

    let emu = emulator();
    for Row {
        case,
        query,
        admits,
    } in rows
    {
        let reply = list(&emu, query).await;
        let actual = listed_ids(case, &reply);
        let expected = expected_ids(&admits);
        assert_eq!(
            actual, expected,
            "case {case:?} ({query:?}): expected the models {expected:?}, got {actual:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 4. A value no filter member admits is refused, never listed around.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_filter_value_the_model_does_not_admit_is_refused() {
    // (case, query) — a value outside each enum's value set, the right value in the wrong
    // case, and a `byProvider` outside `Provider`'s `^[A-Za-z0-9- ]{1,63}$`.
    let rows: &[(&str, &str)] = &[
        ("an output modality outside ModelModality", "byOutputModality=AUDIO"),
        (
            "an output modality in the wrong case",
            "byOutputModality=text",
        ),
        (
            "a customization outside ModelCustomization",
            "byCustomizationType=PROMPT_TUNING",
        ),
        (
            "a customization in the wrong case",
            "byCustomizationType=fine_tuning",
        ),
        (
            "an inference type outside InferenceType",
            "byInferenceType=ONDEMAND",
        ),
        (
            "a provider carrying a character the pattern excludes",
            "byProvider=Anthropic.",
        ),
        (
            "a provider longer than the pattern admits",
            "byProvider=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ),
    ];

    let emu = emulator();
    for (case, query) in rows {
        let reply = list(&emu, query).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?} ({query:?}): expected 400, got {} {:?}",
            reply.status, reply.body
        );
        let code = reply.error.as_deref().map(error_code).unwrap_or("none");
        assert_eq!(
            code, "ValidationException",
            "case {case:?} ({query:?}): expected ValidationException, got {code:?} {:?}",
            reply.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 5. Listing is a pure read.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let emu = emulator();
    // A resource of this service beside the catalog, so a listing that wrote anything
    // would be visible in more than the listing itself.
    let guardrail_id = emu.create_named("catalog-neighbour").await;

    for query in ["", "byProvider=Amazon", "byOutputModality=TEXT"] {
        let first = list(&emu, query).await;
        let second = list(&emu, query).await;
        assert_eq!(
            first.body, second.body,
            "case {query:?}: two identical listings must answer identically"
        );
    }

    let guardrail = emu.get(&guardrail_id, None).await;
    assert_eq!(
        guardrail.status, 200,
        "case \"neighbour\": the guardrail beside the catalog must still read back, got {} {:?}",
        guardrail.status, guardrail.body
    );
}

// ---------------------------------------------------------------------------------------
// 6. Every catalog row is modelled data, and the catalog spans every filterable value.
// ---------------------------------------------------------------------------------------

#[test]
fn every_catalog_row_carries_only_values_the_model_declares() {
    let mut seen_ids: Vec<&str> = Vec::new();
    for model in FOUNDATION_MODELS {
        let case = model.model_id;
        assert!(
            !seen_ids.contains(&model.model_id),
            "case {case:?}: a modelId appears twice in the catalog"
        );
        seen_ids.push(model.model_id);

        assert!(
            (1..=MODEL_ID_MAX_LENGTH).contains(&model.model_id.len())
                && model.model_id.contains('.')
                && model
                    .model_id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-.:".contains(&b)),
            "case {case:?}: a modelId must satisfy BedrockModelId, got {:?}",
            model.model_id
        );
        for (member, name) in [
            ("modelName", model.model_name),
            ("providerName", model.provider_name),
        ] {
            assert!(
                BRANDED_NAME_LENGTH.contains(&name.len()),
                "case {case:?}: {member} must satisfy BrandedName's length {BRANDED_NAME_LENGTH:?}, \
                 got {name:?} ({} characters)",
                name.len()
            );
        }
        // `byProvider` must be able to NAME the provider, so the branded name stays inside
        // `Provider`'s own pattern.
        assert!(
            model
                .provider_name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b' '),
            "case {case:?}: providerName must satisfy Provider's pattern, got {:?}",
            model.provider_name
        );

        assert!(
            !model.input_modalities.is_empty()
                && model
                    .input_modalities
                    .iter()
                    .all(|value| carries(MODEL_MODALITY_VALUES, value)),
            "case {case:?}: inputModalities must be non-empty ModelModality values, got {:?}",
            model.input_modalities
        );
        assert!(
            !model.output_modalities.is_empty()
                && model
                    .output_modalities
                    .iter()
                    .all(|value| carries(MODEL_MODALITY_VALUES, value)),
            "case {case:?}: outputModalities must be non-empty ModelModality values, got {:?}",
            model.output_modalities
        );
        assert!(
            model
                .customizations_supported
                .iter()
                .all(|value| carries(MODEL_CUSTOMIZATION_VALUES, value)),
            "case {case:?}: customizationsSupported must be ModelCustomization values, got {:?}",
            model.customizations_supported
        );
        assert!(
            !model.inference_types_supported.is_empty()
                && model
                    .inference_types_supported
                    .iter()
                    .all(|value| carries(INFERENCE_TYPE_VALUES, value)),
            "case {case:?}: inferenceTypesSupported must be non-empty InferenceType values, got \
             {:?}",
            model.inference_types_supported
        );
        assert!(
            carries(FOUNDATION_MODEL_LIFECYCLE_STATUS_VALUES, model.lifecycle_status),
            "case {case:?}: modelLifecycle.status must be a FoundationModelLifecycleStatus value, \
             got {:?}",
            model.lifecycle_status
        );
    }
}

#[test]
fn the_catalog_spans_every_value_a_caller_can_filter_on() {
    /// Whether a catalog row carries one value of the member under test.
    type Carried = Box<dyn Fn(&FoundationModel, &str) -> bool>;
    /// One member a caller can filter on: its name, the values the model declares for it,
    /// and how a row carries one.
    type Check = (&'static str, &'static [&'static str], Carried);

    let checks: Vec<Check> = vec![
        (
            "byOutputModality",
            MODEL_MODALITY_VALUES,
            Box::new(|model: &FoundationModel, value: &str| carries(model.output_modalities, value)),
        ),
        (
            "byCustomizationType",
            MODEL_CUSTOMIZATION_VALUES,
            Box::new(|model: &FoundationModel, value: &str| {
                carries(model.customizations_supported, value)
            }),
        ),
        (
            "byInferenceType",
            INFERENCE_TYPE_VALUES,
            Box::new(|model: &FoundationModel, value: &str| {
                carries(model.inference_types_supported, value)
            }),
        ),
        (
            "modelLifecycle.status",
            FOUNDATION_MODEL_LIFECYCLE_STATUS_VALUES,
            Box::new(|model: &FoundationModel, value: &str| model.lifecycle_status == value),
        ),
    ];

    for (member, values, carried) in checks {
        for value in values {
            let count = FOUNDATION_MODELS
                .iter()
                .filter(|model| carried(model, value))
                .count();
            assert!(
                count > 0,
                "case {member:?}={value:?}: no catalog row carries this value, so a filter on it \
                 asserts nothing"
            );
        }
    }
}
