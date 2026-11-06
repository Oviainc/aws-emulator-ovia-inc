// SPDX-License-Identifier: CC0-1.0

//! The two catalogues Amazon Bedrock PUBLISHES itself — the `default` prompt routers and
//! the `SYSTEM_DEFINED` cross-region inference profiles — read end to end over the same
//! `MultiServiceRouter` the emulator boots.
//!
//! # The rule
//!
//! A resource Amazon Bedrock publishes exists in every account without any call creating
//! it, so a read of it answers out of the published catalogue rather than out of what a
//! caller created, in whichever region it is offered. There is no `CreatePromptRouter` for
//! a `default` router and no operation at all that mints a `SYSTEM_DEFINED` profile
//! (`PromptRouterType` is `[custom, default]` and `ListPromptRouters$type` defaults to
//! `default`; `InferenceProfileType`'s `SYSTEM_DEFINED` is "defined by Amazon Bedrock" —
//! `smithy_query(query="operation",operation="ListPromptRouters,GetInferenceProfile")` and
//! `query="shape",shape="InferenceProfileSummary"`), so a caller DISCOVERS both and builds
//! its own resources from them: a custom router out of a default router's routable models,
//! and an application inference profile out of a system-defined profile's ARN
//! (`CreateInferenceProfile.modelSource.copyFrom`).
//!
//! # What the tables below generalise over
//!
//! The rows differ along what the rule spans: the region a request addresses (one of each
//! published geography, and a region belonging to none), the spelling an identifier is
//! submitted in (a bare id and an ARN), which published row is read (every one, including
//! rows the issue did not cite), and the type a listing asks for. The catalogue entries
//! these rows name come from the pages AWS publishes them on — "Intelligent prompt
//! routing" and "Supported Regions and models for inference profiles" — and each row's
//! model ids are ids `ListFoundationModels` reports, which this suite checks rather than
//! assumes.

use serde_json::{json, Value};

use crate::support::{emulator, label, profile_model_id, Emulator, Reply, ACCOUNT, REGION};

/// The most tags one resource of this service may carry, as `TooManyTagsException`
/// documents it ("50 tags per resource").
const MAX_TAGS: usize = 50;

/// `InferenceProfileType` / `InferenceProfileStatus` for a published profile.
const SYSTEM_DEFINED: &str = "SYSTEM_DEFINED";
const PROFILE_ACTIVE: &str = "ACTIVE";

/// `PromptRouterType` / `PromptRouterStatus` for a published router.
const TYPE_DEFAULT: &str = "default";
const TYPE_CUSTOM: &str = "custom";
const ROUTER_AVAILABLE: &str = "AVAILABLE";

// ---------------------------------------------------------------------------------------
// Request helpers.
// ---------------------------------------------------------------------------------------

async fn list_routers(emu: &Emulator, region: &str, query: &str) -> Reply {
    let path = if query.is_empty() {
        "/prompt-routers".to_string()
    } else {
        format!("/prompt-routers?{query}")
    };
    emu.call_in_region(region, "GET", &path, None).await
}

async fn get_router(emu: &Emulator, region: &str, arn: &str) -> Reply {
    emu.call_in_region(
        region,
        "GET",
        &format!("/prompt-routers/{}", label(arn)),
        None,
    )
    .await
}

async fn list_foundation_models(emu: &Emulator, region: &str) -> Reply {
    emu.call_in_region(region, "GET", "/foundation-models", None)
        .await
}

async fn list_profiles(emu: &Emulator, region: &str, query: &str) -> Reply {
    let path = if query.is_empty() {
        "/inference-profiles".to_string()
    } else {
        format!("/inference-profiles?{query}")
    };
    emu.call_in_region(region, "GET", &path, None).await
}

async fn get_profile(emu: &Emulator, region: &str, identifier: &str) -> Reply {
    emu.call_in_region(
        region,
        "GET",
        &format!("/inference-profiles/{}", label(identifier)),
        None,
    )
    .await
}

async fn create_profile(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(region, "POST", "/inference-profiles", Some(body))
        .await
}

/// `ListTagsForResource` is bound to `POST /listTagsForResource`.
async fn list_tags(emu: &Emulator, region: &str, resource_arn: &str) -> Reply {
    emu.call_in_region(
        region,
        "POST",
        "/listTagsForResource",
        Some(json!({ "resourceARN": resource_arn })),
    )
    .await
}

/// `n` distinct tags, each key and value inside `TagKey`/`TagValue`'s own pattern.
fn many_tags(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|index| json!({ "key": format!("key-{index}"), "value": format!("value-{index}") }))
            .collect(),
    )
}

/// The `modelId`s `ListFoundationModels` reports in `region`.
async fn published_model_ids(emu: &Emulator, region: &str) -> Vec<String> {
    let listed = list_foundation_models(emu, region).await;
    assert_eq!(
        listed.status, 200,
        "case {region:?}: ListFoundationModels must answer 200, got {} {:?}",
        listed.status, listed.body
    );
    listed
        .get("modelSummaries")
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|row| {
            row.get("modelId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `modelId` a `FoundationModelArn` carries.
fn model_id_of(model_arn: &str) -> String {
    model_arn
        .rsplit_once("foundation-model/")
        .map(|(_, id)| id.to_string())
        .unwrap_or_default()
}

/// The `inferenceProfileArn`s `ListInferenceProfiles` reports in `region` for the profiles
/// Amazon Bedrock publishes there — what a router's target must be one of.
///
/// The base `modelId` behind one of those ARNs is read with `crate::support`'s
/// [`profile_model_id`], which this suite and `tests/all/default_prompt_routers.rs` share,
/// so the two cannot drift on how a published profile names its model.
async fn published_profile_arns(emu: &Emulator, region: &str) -> Vec<String> {
    let listed = list_profiles(emu, region, &format!("type={SYSTEM_DEFINED}")).await;
    assert_eq!(
        listed.status, 200,
        "case {region:?}: ListInferenceProfiles must answer 200, got {} {:?}",
        listed.status, listed.body
    );
    rows(&listed, "inferenceProfileSummaries")
        .iter()
        .map(|row| text(row, "inferenceProfileArn"))
        .collect()
}

/// The summaries a listing reported.
fn rows(reply: &Reply, member: &str) -> Vec<Value> {
    reply
        .get(member)
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("{member} must be a present array, got {:?}", reply.body))
}

/// The string member `name` of one summary row.
fn text(row: &Value, name: &str) -> String {
    row.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------------------
// 1. The published default prompt routers.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_default_listing_reports_the_published_routers_of_the_region() {
    let emu = emulator();
    // The rows are regions a request may address; the published routers are offered in
    // each of them, so each row's listing carries the same routers under that region's own
    // ARNs.
    for region in [REGION, "us-east-1", "eu-west-1", "ap-southeast-2"] {
        let listed = list_routers(&emu, region, "type=default").await;
        assert_eq!(
            listed.status, 200,
            "case {region:?}: ListPromptRouters must answer 200, got {} {:?}",
            listed.status, listed.body
        );
        let summaries = rows(&listed, "promptRouterSummaries");
        assert!(
            !summaries.is_empty(),
            "case {region:?}: expected at least one default prompt router in this \
             account/region, got {:?}",
            listed.body
        );

        let catalog = published_model_ids(&emu, region).await;
        let expected_prefix = format!("arn:aws:bedrock:{region}:{ACCOUNT}:default-prompt-router/");
        let mut seen_arns: Vec<String> = Vec::new();
        for summary in &summaries {
            let arn = text(summary, "promptRouterArn");
            let case = format!("{region} / {arn}");
            assert!(
                arn.starts_with(&expected_prefix),
                "case {case:?}: a published router is addressed in the region it is offered \
                 in, so its ARN must be under {expected_prefix:?}"
            );
            assert_eq!(
                text(summary, "type"),
                TYPE_DEFAULT,
                "case {case:?}: a published router reports the default type"
            );
            assert_eq!(
                text(summary, "status"),
                ROUTER_AVAILABLE,
                "case {case:?}: a published router is serving"
            );
            assert!(
                !text(summary, "promptRouterName").is_empty(),
                "case {case:?}: a summary names the router"
            );
            assert!(
                summary
                    .get("routingCriteria")
                    .and_then(|criteria| criteria.get("responseQualityDifference"))
                    .and_then(Value::as_f64)
                    .is_some(),
                "case {case:?}: routingCriteria is @required, got {summary:?}"
            );

            // The models a caller may route to, and the fallback: each one is reported as
            // the ARN of the CROSS-REGION INFERENCE PROFILE this control plane itself
            // publishes for that model in this region, which is what makes the target
            // routable here.
            let models = summary
                .get("models")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                !models.is_empty(),
                "case {case:?}: a published router reports the models it routes between, \
                 got {summary:?}"
            );
            let fallback = summary
                .get("fallbackModel")
                .and_then(|model| model.get("modelArn"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let profiles = published_profile_arns(&emu, region).await;
            for model_arn in models
                .iter()
                .map(|model| {
                    model
                        .get("modelArn")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .chain(std::iter::once(fallback))
            {
                assert!(
                    model_arn.starts_with(&format!(
                        "arn:aws:bedrock:{region}:{ACCOUNT}:inference-profile/"
                    )),
                    "case {case:?}: a target model is reported as the inference-profile ARN \
                     of this region, got {model_arn:?}"
                );
                assert!(
                    profiles.contains(&model_arn),
                    "case {case:?}: the target {model_arn:?} must be a profile \
                     ListInferenceProfiles reports, which carries {profiles:?}"
                );
                let model_id = profile_model_id(&model_arn);
                assert!(
                    catalog.contains(&model_id),
                    "case {case:?}: the target model {model_id:?} must be one \
                     ListFoundationModels reports, which carries {catalog:?}"
                );
            }

            // The read of that one router reports the same record the listing does.
            let read = get_router(&emu, region, &arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: GetPromptRouter must answer 200 for a published router, got \
                 {} {:?}",
                read.status, read.body
            );
            for member in [
                "promptRouterName",
                "promptRouterArn",
                "routingCriteria",
                "description",
                "createdAt",
                "updatedAt",
                "models",
                "fallbackModel",
                "status",
                "type",
            ] {
                assert_eq!(
                    summary.get(member).cloned().unwrap_or(Value::Null),
                    read.get(member),
                    "case {case:?} / {member:?}: the summary and the read must agree, got \
                     {:?} and {:?}",
                    summary.get(member),
                    read.get(member)
                );
            }
            for member in ["description", "createdAt", "updatedAt"] {
                assert_ne!(
                    summary.get(member).cloned().unwrap_or(Value::Null),
                    Value::Null,
                    "case {case:?}: a published router carries {member}, got {summary:?}"
                );
            }
            seen_arns.push(arn);
        }

        // The published routers are `default`, so the `custom` listing of an account that
        // created none is empty.
        let custom = list_routers(&emu, region, "type=custom").await;
        assert_eq!(
            rows(&custom, "promptRouterSummaries"),
            Vec::<Value>::new(),
            "case {region:?}: a published router is not a custom one, got {:?}",
            custom.body
        );

        // And the ARNs are distinct: one entry per published router.
        let mut sorted = seen_arns.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            seen_arns.len(),
            "case {region:?}: each published router is listed once, got {seen_arns:?}"
        );
    }
}

#[tokio::test]
async fn a_custom_router_is_built_from_a_published_routers_models() {
    // The workflow the catalogue exists for: discover a default router, then create a
    // router of your own over the models it reports.
    let emu = emulator();
    let listed = list_routers(&emu, REGION, "type=default").await;
    let first = rows(&listed, "promptRouterSummaries")
        .first()
        .cloned()
        .expect("the default catalogue carries at least one router");

    let created = emu
        .call(
            "POST",
            "/prompt-routers",
            Some(json!({
                "promptRouterName": "built from a default router",
                "models": first.get("models").cloned().unwrap_or(Value::Null),
                "fallbackModel": first.get("fallbackModel").cloned().unwrap_or(Value::Null),
                "routingCriteria": { "responseQualityDifference": 5.0 },
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"a router over the published models\": CreatePromptRouter must be admitted, \
         got {} {:?}",
        created.status, created.body
    );
    let arn = created.text("promptRouterArn");
    let read = get_router(&emu, REGION, &arn).await;
    assert_eq!(
        read.get("models"),
        first.get("models").cloned().unwrap_or(Value::Null),
        "case \"a router over the published models\": the created router routes to the models \
         the request named, got {:?}",
        read.body
    );
    assert_eq!(
        read.get("type"),
        json!(TYPE_CUSTOM),
        "case \"a router over the published models\": a router a caller created is custom, \
         got {:?}",
        read.body
    );
}

// ---------------------------------------------------------------------------------------
// 2. The published system-defined inference profiles.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_published_profile_resolves_under_both_identifier_spellings() {
    let emu = emulator();
    // One region of each published geography, so the rows span the geographies rather than
    // the one the issue cited.
    for region in [REGION, "eu-central-1", "ap-northeast-1"] {
        let listed = list_profiles(&emu, region, &format!("type={SYSTEM_DEFINED}")).await;
        assert_eq!(
            listed.status, 200,
            "case {region:?}: ListInferenceProfiles must answer 200, got {} {:?}",
            listed.status, listed.body
        );
        let summaries = rows(&listed, "inferenceProfileSummaries");
        assert!(
            !summaries.is_empty(),
            "case {region:?}: expected the published cross-region profiles of this region, \
             got {:?}",
            listed.body
        );

        for summary in &summaries {
            let id = text(summary, "inferenceProfileId");
            let arn = text(summary, "inferenceProfileArn");
            // EVERY published row is read, under BOTH spellings the identifier admits.
            for (spelling, identifier) in [("the bare id", &id), ("the profile ARN", &arn)] {
                let case = format!("{region} / {id} / {spelling}");
                let read = get_profile(&emu, region, identifier).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: GetInferenceProfile must answer 200 for a published \
                     profile, got {} {:?}",
                    read.status, read.body
                );
                assert_eq!(
                    read.get("type"),
                    json!(SYSTEM_DEFINED),
                    "case {case:?}: a published profile reports the system-defined type, got \
                     {:?}",
                    read.body
                );
                assert_eq!(
                    read.get("status"),
                    json!(PROFILE_ACTIVE),
                    "case {case:?}: a published profile is ready to be used, got {:?}",
                    read.body
                );
                assert_eq!(
                    read.text("inferenceProfileArn"),
                    arn,
                    "case {case:?}: both spellings address the one profile"
                );
                for member in [
                    "inferenceProfileName",
                    "inferenceProfileId",
                    "description",
                    "createdAt",
                    "updatedAt",
                    "models",
                    "status",
                    "type",
                ] {
                    assert_eq!(
                        summary.get(member).cloned().unwrap_or(Value::Null),
                        read.get(member),
                        "case {case:?} / {member:?}: the summary and the read must agree, got \
                         {:?} and {:?}",
                        summary.get(member),
                        read.get(member)
                    );
                }
                for member in ["description", "createdAt", "updatedAt"] {
                    assert_ne!(
                        read.get(member),
                        Value::Null,
                        "case {case:?}: a published profile carries {member}, got {:?}",
                        read.body
                    );
                }
            }

            // A published id is read only in a region it is offered in: the profile of
            // another geography's region is another account-region's resource.
            let elsewhere = get_profile(&emu, "sa-east-1", &id).await;
            assert_eq!(
                elsewhere.status, 404,
                "case {region:?} / {id:?}: a region with no published profile of this id must \
                 answer 404, got {} {:?}",
                elsewhere.status, elsewhere.body
            );
        }
    }
}

#[tokio::test]
async fn an_application_profile_is_created_from_a_published_profile_and_tagged() {
    let emu = emulator();
    let listed = list_profiles(&emu, REGION, &format!("type={SYSTEM_DEFINED}")).await;
    let published = rows(&listed, "inferenceProfileSummaries");
    let source = text(
        published
            .first()
            .expect("the published catalogue carries at least one profile"),
        "inferenceProfileArn",
    );

    // More tags than a resource may carry: refused with the modelled error, whose message
    // states the bound and nothing about the request — and nothing is created.
    let refused = create_profile(
        &emu,
        REGION,
        json!({
            "inferenceProfileName": "too many tags",
            "modelSource": { "copyFrom": source },
            "tags": many_tags(MAX_TAGS + 10),
        }),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error.as_deref()),
        (400, Some("TooManyTagsException")),
        "case \"more tags than the bound\": expected 400 TooManyTagsException, got {} {:?}",
        refused.status,
        refused.body
    );
    let expected = format!("You can assign a maximum of {MAX_TAGS} tags to a resource");
    assert_eq!(
        refused.body.get("message").and_then(Value::as_str),
        Some(expected.as_str()),
        "case \"more tags than the bound\": expected the message {expected:?}, got {:?}",
        refused.body
    );
    let after = list_profiles(&emu, REGION, "type=APPLICATION").await;
    assert_eq!(
        rows(&after, "inferenceProfileSummaries"),
        Vec::<Value>::new(),
        "case \"more tags than the bound\": a refused create stores nothing, got {:?}",
        after.body
    );

    // The same request inside the bound is accepted, is ACTIVE, and its tags are readable.
    let accepted = create_profile(
        &emu,
        REGION,
        json!({
            "inferenceProfileName": "copied from a published profile",
            "modelSource": { "copyFrom": source },
            "tags": many_tags(1),
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 201,
        "case \"a create inside the bound\": CreateInferenceProfile must answer 201, got {} \
         {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        accepted.get("status"),
        json!(PROFILE_ACTIVE),
        "case \"a create inside the bound\": expected {PROFILE_ACTIVE:?}, got {:?}",
        accepted.body
    );
    let created_arn = accepted.text("inferenceProfileArn");
    let tags = list_tags(&emu, REGION, &created_arn).await;
    assert_eq!(
        tags.get("tags"),
        many_tags(1),
        "case \"a create inside the bound\": the tags the create carried must be readable, \
         got {:?}",
        tags.body
    );

    // The created profile tracks the base model the published source routes to, and is an
    // APPLICATION profile rather than a published one.
    let read = get_profile(&emu, REGION, &created_arn).await;
    assert_eq!(
        read.get("type"),
        json!("APPLICATION"),
        "case \"a create inside the bound\": a profile a caller created is APPLICATION, got \
         {:?}",
        read.body
    );
    let tracked = read
        .get("models")
        .as_array()
        .and_then(|models| models.first())
        .and_then(|model| model.get("modelArn"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let catalog = published_model_ids(&emu, REGION).await;
    assert!(
        catalog.contains(&model_id_of(&tracked)),
        "case \"a create inside the bound\": the profile tracks a model this control plane \
         publishes, got {tracked:?} against {catalog:?}"
    );
}
