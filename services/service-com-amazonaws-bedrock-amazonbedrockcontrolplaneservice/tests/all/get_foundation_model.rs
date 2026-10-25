// SPDX-License-Identifier: CC0-1.0

//! `GetFoundationModel` end-to-end, over the same `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A read reports the ONE catalog row the identifier names**: every member of
//!    `modelDetails` is read off that row of the published catalog the store is seeded
//!    with, so the response is computed from state rather than written per request. The
//!    rows of the table are EVERY model the catalog offers, which is what makes it span
//!    providers, modalities, streaming support, customizations, inference types and both
//!    lifecycle statuses.
//! 2. **Both spellings of an identifier name the same model**: `modelIdentifier` admits
//!    the bare `modelId` and the model's `foundation-model` ARN
//!    (`smithy_query(query="shape",shape="GetFoundationModelIdentifier")`), and the two
//!    answer identically.
//! 3. **The reported ARN is the `foundation-model` ARN for the region the request
//!    addresses**, with the EMPTY account field `FoundationModelArn` declares, and the
//!    catalog itself does not vary by region.
//! 4. **An identifier that names no model the service offers is refused with 404
//!    `ResourceNotFoundException`** — a bare id and an ARN of a model the catalog has no
//!    row for, including the ids of models past their end of life, which are offered by
//!    nobody and so appear in no listing either.
//! 5. **A value the identifier's own constraints exclude is refused with 400
//!    `ValidationException`**, never read around and never reported as a missing resource.
//! 6. **Reading is a pure read**: two reads of the same identifier answer identically, a
//!    read changes no later caller's listing, and a resource beside the catalog is
//!    untouched.
//! 7. **The read and the listing agree member by member**: the details of a model are the
//!    summary `ListFoundationModels` reports for it, because both are read off the same
//!    row.
//!
//! Expectations come from the model
//! (`smithy_query(query="operation",operation="GetFoundationModel")` and `query="shape"`
//! on `FoundationModelDetails`, `FoundationModelLifecycle` and
//! `GetFoundationModelIdentifier`), and the requests go through `crate::support` — the ONE
//! harness every suite in this crate drives the emulator with.

use serde_json::{json, Value};

use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::{
    FoundationModel, FOUNDATION_MODELS,
};

use crate::support::{emulator, error_code, foundation_model_arn as model_arn, label, Emulator, Reply, REGION};

/// The path `GetFoundationModel` is bound to
/// (`smithy_query(query="operation",operation="GetFoundationModel")`: `GET
/// /foundation-models/{modelIdentifier}`).
const PATH: &str = "/foundation-models";

/// Model ids AWS publishes as past their end of life ("Model lifecycle" in the Amazon
/// Bedrock User Guide), which this emulator's inference plane keeps in its own inventory
/// in order to refuse them. None of them is a model the service still offers, so the
/// control plane carries no row for any of them — the rows below span four providers and
/// both text and image models, and only one of them is the id this issue's check reads.
const END_OF_LIFE_MODEL_IDS: &[&str] = &[
    "amazon.titan-text-express-v1",
    "anthropic.claude-v2",
    "anthropic.claude-instant-v1",
    "ai21.j2-ultra-v1",
    "meta.llama2-70b-chat-v1",
    "stability.stable-diffusion-xl-v0",
];

/// One read of `identifier`, signed for `region`.
async fn get_in_region(emu: &Emulator, region: &str, identifier: &str) -> Reply {
    emu.call_in_region(region, "GET", &format!("{PATH}/{}", label(identifier)), None)
        .await
}

/// One read of `identifier`, signed for the suite's own region.
async fn get(emu: &Emulator, identifier: &str) -> Reply {
    get_in_region(emu, REGION, identifier).await
}

/// The `modelDetails` a read reported.
fn details(case: &str, reply: &Reply) -> Value {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: a read of an offered model must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    let value = reply.get("modelDetails");
    assert!(
        value.is_object(),
        "case {case:?}: modelDetails must be a present structure, got {:?}",
        reply.body
    );
    value
}

/// The `FoundationModelDetails` a read of `model` in `region` must report, read off the
/// catalog row itself.
fn expected_details(model: &FoundationModel, region: &str) -> Value {
    json!({
        "modelArn": format!("arn:aws:bedrock:{region}::foundation-model/{}", model.model_id),
        "modelId": model.model_id,
        "modelName": model.model_name,
        "providerName": model.provider_name,
        "inputModalities": model.input_modalities,
        "outputModalities": model.output_modalities,
        "responseStreamingSupported": model.response_streaming_supported,
        "customizationsSupported": model.customizations_supported,
        "inferenceTypesSupported": model.inference_types_supported,
        // Only `status`: no page publishes a launch, legacy or end-of-life instant per
        // model, so a read reports none of the four.
        "modelLifecycle": { "status": model.lifecycle_status },
    })
}

// ---------------------------------------------------------------------------------------
// 1 and 2. A read reports the row the identifier names, by either spelling.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn every_offered_model_reads_back_as_its_catalog_row() {
    let emu = emulator();
    for model in FOUNDATION_MODELS {
        let case = model.model_id;
        let reply = get(&emu, model.model_id).await;
        let actual = details(case, &reply);
        let expected = expected_details(model, REGION);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the details {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_and_a_bare_id_name_the_same_model() {
    let emu = emulator();
    for model in FOUNDATION_MODELS {
        let case = model.model_id;
        let by_id = get(&emu, model.model_id).await;
        let by_arn = get(&emu, &model_arn(REGION, model.model_id)).await;
        assert_eq!(
            details(case, &by_arn),
            details(case, &by_id),
            "case {case:?}: the ARN and the bare id must name the same model"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. The reported ARN carries the region the request addresses, and no account.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_read_reports_the_arn_of_the_region_the_request_addresses() {
    // The rows differ along the region the request signs for: the suite's own, another
    // partition-mate, and one in a third geography.
    let regions = ["us-west-2", "eu-central-1", "ap-southeast-2"];

    let emu = emulator();
    for region in regions {
        for model in FOUNDATION_MODELS {
            let case = format!("{region} {}", model.model_id);
            let reply = get_in_region(&emu, region, model.model_id).await;
            let actual = details(&case, &reply);
            assert_eq!(
                actual,
                expected_details(model, region),
                "case {case:?}: the catalog must not vary by region, and the ARN must carry \
                 the region the request addresses"
            );
            let arn = actual
                .get("modelArn")
                .and_then(Value::as_str)
                .unwrap_or_default();
            // The account field of a `foundation-model` ARN is empty: the model is
            // published by AWS rather than owned by the account reading it.
            let account = arn.split(':').nth(4).unwrap_or("missing");
            assert_eq!(
                account, "",
                "case {case:?}: a foundation-model ARN carries no account, got {account:?} in \
                 {arn:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 4. An identifier naming no offered model is refused with 404.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_identifier_naming_no_offered_model_is_not_found() {
    // The rows differ along WHY the identifier names no offered model: a provider the
    // catalog has no row from, a known provider with an unknown model, a model past its
    // end of life (by bare id and by ARN), and an ARN whose model the catalog has no row
    // for — including one minted for another region.
    let mut rows: Vec<(String, String)> = vec![
        (
            "a provider the catalog carries nothing from".to_string(),
            "nonesuch.model-v1:0".to_string(),
        ),
        (
            "a known provider with a model the catalog has no row for".to_string(),
            "amazon.nova-sonic-v9:0".to_string(),
        ),
        (
            "an ARN of a model the catalog has no row for".to_string(),
            model_arn(REGION, "nonesuch.model-v1:0"),
        ),
        (
            "an ARN from another region of a model the catalog has no row for".to_string(),
            model_arn("eu-central-1", "nonesuch.model-v1:0"),
        ),
    ];
    for model_id in END_OF_LIFE_MODEL_IDS {
        rows.push((
            format!("the end-of-life model {model_id}, by bare id"),
            (*model_id).to_string(),
        ));
        rows.push((
            format!("the end-of-life model {model_id}, by ARN"),
            model_arn(REGION, model_id),
        ));
    }

    let emu = emulator();
    for (case, identifier) in rows {
        let reply = get(&emu, &identifier).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?} ({identifier:?}): expected 404, got {} {:?}",
            reply.status, reply.body
        );
        let code = reply.error.as_deref().map(error_code).unwrap_or("none");
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?} ({identifier:?}): expected ResourceNotFoundException, got {code:?} \
             {:?}",
            reply.body
        );
    }
}

/// A model nothing offers is in no listing either: the catalog a read resolves against and
/// the catalog a listing reports are the same one.
#[tokio::test]
async fn an_end_of_life_model_appears_in_no_listing() {
    let emu = emulator();
    let reply = emu.call("GET", PATH, None).await;
    assert_eq!(
        reply.status, 200,
        "case \"listing\": a listing must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    let listed: Vec<String> = reply
        .get("modelSummaries")
        .as_array()
        .unwrap_or_else(|| panic!("case \"listing\": modelSummaries must be an array: {:?}", reply.body))
        .iter()
        .filter_map(|row| row.get("modelId").and_then(Value::as_str).map(str::to_string))
        .collect();
    for model_id in END_OF_LIFE_MODEL_IDS {
        assert!(
            !listed.contains(&(*model_id).to_string()),
            "case {model_id:?}: a model past its end of life is offered by nobody, so it must \
             appear in no listing, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 5. A value the identifier's constraints exclude is refused with 400.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_identifier_outside_its_declared_form_is_refused() {
    // `GetFoundationModelIdentifier` is len 1..2048 and anchored on either a
    // `foundation-model` ARN or a dotted, lowercase bare model id, so each row below is a
    // value one of those constraints excludes: no dot, upper case, a character outside the
    // class, another service's ARN, and a value longer than the declared maximum.
    let long_id = format!("amazon.{}", "a".repeat(2048));
    let rows: Vec<(&str, String)> = vec![
        ("no provider segment at all", "novalite".to_string()),
        ("upper case, which the pattern excludes", "Amazon.Nova-Lite-v1:0".to_string()),
        (
            "a character the pattern excludes",
            "amazon.nova_lite_v1".to_string(),
        ),
        (
            "another service's ARN",
            format!("arn:aws:sagemaker:{REGION}:000000000000:endpoint/nova"),
        ),
        ("longer than the declared maximum", long_id),
    ];

    let emu = emulator();
    for (case, identifier) in rows {
        let reply = get(&emu, &identifier).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?} ({identifier:?}): expected 400, got {} {:?}",
            reply.status, reply.body
        );
        let code = reply.error.as_deref().map(error_code).unwrap_or("none");
        assert_eq!(
            code, "ValidationException",
            "case {case:?} ({identifier:?}): expected ValidationException, got {code:?} {:?}",
            reply.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 6. Reading is a pure read.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn reading_changes_nothing_a_later_caller_sees() {
    let emu = emulator();
    // A resource of this service beside the catalog, so a read that wrote anything would
    // be visible in more than the read itself.
    let guardrail_id = emu.create_named("catalog-reader-neighbour").await;
    let before = emu.call("GET", PATH, None).await;

    for model in FOUNDATION_MODELS {
        let case = model.model_id;
        let first = get(&emu, model.model_id).await;
        let second = get(&emu, model.model_id).await;
        assert_eq!(
            first.body, second.body,
            "case {case:?}: two identical reads must answer identically"
        );
    }
    // A refusal must not write either.
    let _ = get(&emu, "nonesuch.model-v1:0").await;

    let after = emu.call("GET", PATH, None).await;
    assert_eq!(
        before.body, after.body,
        "case \"listing\": reads must leave the catalog a later caller lists unchanged"
    );
    let guardrail = emu.get(&guardrail_id, None).await;
    assert_eq!(
        guardrail.status, 200,
        "case \"neighbour\": the guardrail beside the catalog must still read back, got {} {:?}",
        guardrail.status, guardrail.body
    );
}

// ---------------------------------------------------------------------------------------
// 7. The read and the listing agree, member by member.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_read_reports_what_the_listing_reports_for_the_same_model() {
    let emu = emulator();
    let listing = emu.call("GET", PATH, None).await;
    let rows = listing
        .get("modelSummaries")
        .as_array()
        .unwrap_or_else(|| panic!("case \"listing\": modelSummaries must be an array: {:?}", listing.body))
        .clone();
    assert!(
        !rows.is_empty(),
        "case \"listing\": the catalog must not be empty, or this suite asserts nothing"
    );

    for row in &rows {
        let model_id = row
            .get("modelId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case \"listing\": a summary must carry modelId: {row:?}"));
        let reply = get(&emu, model_id).await;
        let actual = details(model_id, &reply);
        assert_eq!(
            &actual, row,
            "case {model_id:?}: the details and the summary are the same row, so they must \
             agree member by member; got {actual:?} against {row:?}"
        );
    }
}
