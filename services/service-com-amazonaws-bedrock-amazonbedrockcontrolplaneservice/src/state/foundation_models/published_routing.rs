// SPDX-License-Identifier: CC0-1.0

//! The catalogues of ROUTING resources Amazon Bedrock publishes itself: the `default`
//! prompt routers and the `SYSTEM_DEFINED` cross-region inference profiles.
//!
//! Both are published data rather than anything a request brings into being — this service
//! has no operation that creates one — so, like the model catalogue they are built over,
//! these rows are the same for every account and no handler writes one.

use super::*;

/// `PromptRouterType` — the type of a router Amazon Bedrock itself publishes.
///
/// The other of the two values the enum carries, beside [`PROMPT_ROUTER_TYPE_CUSTOM`].
pub const PROMPT_ROUTER_TYPE_DEFAULT: &str = "default";

/// The `responseQualityDifference` a published router reports.
///
/// `RoutingCriteria$responseQualityDifference` is modelled `@range(min: 0, max: 100)` and
/// is `@required`, so a published router reports one; AWS publishes no tolerance for its
/// own routers, so this is the minimum that range admits — the strictest criteria, under
/// which a prompt is routed to the smaller model only when its answer is no worse.
pub const PUBLISHED_ROUTING_QUALITY_DIFFERENCE: f64 = 0.0;

/// One prompt router Amazon Bedrock publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultPromptRouter {
    /// The resource half of the router's `PromptRouterArn`, whose pattern admits
    /// `[a-zA-Z0-9-:.]+` there ([`prompt_router_arn`]). Spelled `<provider>.<family>:<n>`,
    /// the form AWS publishes a default router's ARN under.
    pub id: &'static str,
    /// The BRAND the router is published under: the publisher's own name for the model
    /// line it routes within — a third-party provider's name for that provider's line, and
    /// the line's own name for a line AWS publishes itself, because AWS does not brand its
    /// own models by its provider name. It is the same split the published cross-region
    /// profiles name their models by ([`cross_region_model_brand`]).
    ///
    /// The router's `promptRouterName` is built from it ([`default_prompt_router_name`]),
    /// so the brand a router is published under is stated once.
    pub label: &'static str,
    /// The `modelId`s the router routes between, in the order the summary reports them —
    /// the smaller model first, because the criteria are what sends a prompt UP to the
    /// larger one. The set AWS publishes for the router, which is a line's smaller model
    /// and the stronger model a prompt is routed up to; the stronger one is the fallback,
    /// and its published family ([`FoundationModel::model_family`]) is the line the
    /// router's description names. Every id is a row of [`FOUNDATION_MODELS`] that also
    /// carries a cross-region profile ([`CROSS_REGION_MODELS`]), because that profile is
    /// what a target is reported as.
    pub models: &'static [&'static str],
    /// The `modelId` of the model the router falls back to when the criteria are not met;
    /// one of [`DefaultPromptRouter::models`].
    pub fallback_model: &'static str,
}

/// The prompt routers this emulator's Amazon Bedrock publishes, in `id` order — the order
/// their ARNs sort in, which is the order a listing pages through.
///
/// The ids, brands and model sets are the ones AWS publishes in "Intelligent prompt
/// routing": one router per model family, routing between a smaller model of that family
/// and the stronger model it falls back to. The model ids are the rows
/// [`FOUNDATION_MODELS`] carries for those families, so the two catalogs agree about which
/// models exist.
pub const DEFAULT_PROMPT_ROUTERS: &[DefaultPromptRouter] = &[
    DefaultPromptRouter {
        id: "amazon.nova:1",
        label: "Nova",
        models: &["amazon.nova-lite-v1:0", "amazon.nova-pro-v1:0"],
        fallback_model: "amazon.nova-pro-v1:0",
    },
    DefaultPromptRouter {
        id: "anthropic.claude:1",
        label: "Anthropic",
        models: &[
            "anthropic.claude-3-haiku-20240307-v1:0",
            "anthropic.claude-3-5-sonnet-20241022-v2:0",
        ],
        fallback_model: "anthropic.claude-3-5-sonnet-20241022-v2:0",
    },
    DefaultPromptRouter {
        id: "meta.llama:1",
        label: "Meta",
        models: &[
            "meta.llama3-1-8b-instruct-v1:0",
            "meta.llama3-1-70b-instruct-v1:0",
        ],
        fallback_model: "meta.llama3-1-70b-instruct-v1:0",
    },
];

/// The `promptRouterName` a published router reports: the brand it is published under,
/// then what it is.
///
/// `PromptRouterName` is `@required` (len 1..64, `^([0-9a-zA-Z][ _-]?)+$`), so a published
/// router carries one; it is built from [`DefaultPromptRouter::label`] rather than written
/// out per router, so the brand a router is published under is stated once.
#[must_use]
pub fn default_prompt_router_name(router: &DefaultPromptRouter) -> String {
    format!("{} Prompt Router", router.label)
}

/// The `description` a published router reports: the model FAMILY it routes a prompt
/// within.
///
/// `PromptRouterDescription` is modelled (len 1..200, `^([0-9a-zA-Z:.][ _-]?)+$`) and a
/// published router carries one, so it is stated for every row by ONE rule rather than
/// written out per router: what a default router IS, is the published family of the model
/// it routes a prompt UP to ([`FoundationModel::model_family`]) — the stronger model it
/// falls back to, which is the line a caller is choosing when it picks the router.
///
/// `None` for a router whose fallback model the catalogue offers no row for, because the
/// family of a model nothing publishes is not something to invent; such a router publishes
/// no targets either ([`default_prompt_routers`]), so no listing reports it.
#[must_use]
pub fn default_prompt_router_description(router: &DefaultPromptRouter) -> Option<String> {
    let family = catalogued_model(router.fallback_model)?.model_family;
    Some(format!("Routes requests among models in the {family} family"))
}

/// Every prompt router Amazon Bedrock publishes in `region`, as the `PromptRouterSummary`
/// a listing carries, in `promptRouterArn` order.
///
/// Each one reports the models it routes between as `PromptRouterTargetModel`s carrying
/// the ARN of the CROSS-REGION INFERENCE PROFILE of this region's geography for that model
/// ([`system_defined_inference_profiles`]) — the `inference-profile` form
/// `PromptRouterTargetModelArn` admits beside a `FoundationModelArn` — because a default
/// router routes a prompt across the regions of its geography rather than at one region's
/// copy of a model, and the profile is the resource that routing is entitled through.
/// A target therefore names a profile this control plane itself publishes: a router is
/// offered in a region exactly while every model it routes between has a published profile
/// there, which is why a region of no geography publishes none.
///
/// `createdAt` and `updatedAt` are the instant the router came into being, which for a
/// resource no caller created is the day the LAST of the models it routes between launched
/// ([`FoundationModel::start_of_life_time`]): a router cannot route to a model that does
/// not exist yet, and nothing has updated it since, so the two instants agree.
#[must_use]
pub fn default_prompt_routers(region: &str) -> Vec<PromptRouterSummary> {
    let published = system_defined_inference_profiles(region);
    let target = |model_id: &str| {
        let id = cross_region_profile_id(region, model_id)?;
        published
            .iter()
            .find(|profile| profile.inference_profile_id == id)
            .map(|profile| PromptRouterTargetModel {
                model_arn: profile.inference_profile_arn.clone(),
            })
    };
    DEFAULT_PROMPT_ROUTERS
        .iter()
        .filter_map(|router| {
            let models: Vec<PromptRouterTargetModel> = router
                .models
                .iter()
                .map(|model_id| target(model_id))
                .collect::<Option<_>>()?;
            let published_at = router
                .models
                .iter()
                .filter_map(|model_id| catalogued_model(model_id))
                .map(|model| model.start_of_life_time)
                .max()
                .and_then(published_instant);
            Some(PromptRouterSummary {
                prompt_router_name: default_prompt_router_name(router),
                routing_criteria: RoutingCriteria {
                    response_quality_difference: PUBLISHED_ROUTING_QUALITY_DIFFERENCE,
                },
                description: default_prompt_router_description(router),
                created_at: published_at,
                updated_at: published_at,
                prompt_router_arn: prompt_router_arn(region, router.id),
                models,
                fallback_model: target(router.fallback_model)?,
                status: PROMPT_ROUTER_AVAILABLE.to_string(),
                r#type: PROMPT_ROUTER_TYPE_DEFAULT.to_string(),
            })
        })
        .collect()
}

/// The prompt router Amazon Bedrock publishes in `region` under `arn`, if it publishes one.
///
/// `GetPromptRouter` addresses a router by its `PromptRouterArn` alone, and a published
/// router carries the ARN of the region it is offered in, so one of another region names
/// another account-region's router.
#[must_use]
pub fn default_prompt_router(arn: &str, region: &str) -> Option<PromptRouterSummary> {
    default_prompt_routers(region)
        .into_iter()
        .find(|published| published.prompt_router_arn == arn)
}

// =============================================================================
// The published cross-region inference profile catalog
// =============================================================================
//
// Amazon Bedrock publishes the SYSTEM_DEFINED inference profiles itself — "the inference
// profile is defined by Amazon Bedrock. You can route inference requests across regions
// with these inference profiles"
// (`smithy_query(query="shape",shape="InferenceProfileSummary")`) — and there is no
// operation that creates one, so these rows are published data the same way
// [`FOUNDATION_MODELS`] is: the same for every account, written by no handler.

/// `InferenceProfileType` — the type of a profile Amazon Bedrock itself defines.
///
/// The other of the two values `InferenceProfileType` carries, beside
/// [`INFERENCE_PROFILE_TYPE_APPLICATION`].
pub const INFERENCE_PROFILE_TYPE_SYSTEM_DEFINED: &str = "SYSTEM_DEFINED";

/// One geography a cross-region inference profile routes invocations within.
///
/// A geography is the unit the published profiles are cut along: a profile exists once per
/// geography per model, is offered in that geography's regions, and routes requests to
/// those same regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InferenceProfileGeography {
    /// The leading dot-separated segment of the published `inferenceProfileId`
    /// (`us.<modelId>`), which is also what `ops/create_inference_profile.rs` reads back
    /// off such an id to find the model behind it.
    pub prefix: &'static str,
    /// How the published `inferenceProfileName` spells the geography, ahead of the model's
    /// own brand ([`cross_region_model_brand`]).
    pub label: &'static str,
    /// The regions of the geography: where the profile is offered, and where it routes
    /// invocations to. At most five, because `InferenceProfileModels` is
    /// `@length(min: 1, max: 5)` and a published profile reports one model per region.
    pub regions: &'static [&'static str],
}

/// The geographies this emulator's Amazon Bedrock publishes cross-region profiles for, in
/// `prefix` order — the order their profile ids sort in.
///
/// The prefixes and the regions are the ones AWS publishes in "Supported Regions and
/// models for inference profiles": a geography's profile routes to the commercial regions
/// of that geography. A SUBSET of what AWS offers, spanning geographies of three different
/// sizes, because the listing rule — not the catalog's size — is what this emulator has to
/// get right.
pub const INFERENCE_PROFILE_GEOGRAPHIES: &[InferenceProfileGeography] = &[
    InferenceProfileGeography {
        prefix: "apac",
        label: "APAC",
        regions: &[
            "ap-northeast-1",
            "ap-northeast-2",
            "ap-south-1",
            "ap-southeast-1",
            "ap-southeast-2",
        ],
    },
    InferenceProfileGeography {
        prefix: "eu",
        label: "EU",
        regions: &["eu-central-1", "eu-north-1", "eu-west-1", "eu-west-3"],
    },
    InferenceProfileGeography {
        prefix: "us",
        label: "US",
        regions: &["us-east-1", "us-east-2", "us-west-2"],
    },
];

/// The base models this emulator's Amazon Bedrock publishes a cross-region inference
/// profile for, by `modelId`, in `modelId` order.
///
/// The models AWS lists as supported for cross-region inference on "Supported Regions and
/// models for inference profiles", restricted to the ids [`FOUNDATION_MODELS`] carries so
/// no published profile routes to a model this control plane does not offer — an id with
/// no row publishes no profile rather than one naming a model nothing serves. A SUBSET of
/// what AWS offers, spanning three providers, and CLOSED under
/// [`DEFAULT_PROMPT_ROUTERS`]: every model a published router routes between has a profile
/// here, because that profile is what the router reports as its target.
///
/// Only the ids are written here. Everything else a profile reports about its model — the
/// brand its name carries, the instant it came into being — is read off that model's
/// published row, so the two catalogs cannot disagree about one model.
pub const CROSS_REGION_MODELS: &[&str] = &[
    "amazon.nova-lite-v1:0",
    "amazon.nova-micro-v1:0",
    "amazon.nova-pro-v1:0",
    "anthropic.claude-3-5-sonnet-20241022-v2:0",
    "anthropic.claude-3-haiku-20240307-v1:0",
    "anthropic.claude-3-sonnet-20240229-v1:0",
    "meta.llama3-1-70b-instruct-v1:0",
    "meta.llama3-1-8b-instruct-v1:0",
];

/// The BRAND a published cross-region profile names its model by, after the geography's
/// own label.
///
/// A model's brand is what its publisher sells it as: the provider's name followed by the
/// model's name for a model published by a third party, and the model's name alone for one
/// AWS publishes itself ([`AWS_MODEL_PROVIDER`]), because AWS does not brand its own models
/// by its provider name. Both halves are the row's own published members
/// ([`FoundationModel::provider_name`], [`FoundationModel::model_name`]), so a profile
/// names a model exactly as the model catalogue does.
///
/// A brand is reported AS PUBLISHED, version separator included, so a name may carry a `.`
/// that `InferenceProfileName`'s pattern (`^([0-9a-zA-Z][ _-]?)+$`) does not admit. That
/// pattern constrains the name a CALLER may give a profile of its own — the validating walk
/// runs on operation inputs — while this is the name AWS itself publishes for a profile no
/// caller created, and re-spelling a branded name to fit an input constraint would report a
/// name no client could look the model up by.
#[must_use]
pub fn cross_region_model_brand(model: &FoundationModel) -> String {
    if model.provider_name == AWS_MODEL_PROVIDER {
        model.model_name.to_string()
    } else {
        format!("{} {}", model.provider_name, model.model_name)
    }
}

/// The ARN of a system-defined inference profile in `region`:
/// `arn:aws:bedrock:<region>:<account>:inference-profile/<id>`.
///
/// `inference-profile` is the resource type `InferenceProfileArn`'s pattern admits beside
/// `application-inference-profile` (see [`application_inference_profile_arn`]), and it is
/// the one the profiles Amazon Bedrock publishes carry.
#[must_use]
pub fn system_defined_inference_profile_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("inference-profile/{id}"))
}

/// The `inferenceProfileId` of the cross-region profile that routes `model_id` within the
/// geography `region` belongs to (`<prefix>.<modelId>`), or `None` when the catalog
/// publishes no profile in that region.
///
/// Stated once because it is the id BOTH catalogs name a cross-region profile by: a
/// listing builds the published profile under it, and a default router names its target
/// profile by it ([`default_prompt_routers`]).
#[must_use]
pub fn cross_region_profile_id(region: &str, model_id: &str) -> Option<String> {
    inference_profile_geography(region).map(|geography| format!("{}.{model_id}", geography.prefix))
}

/// The geography `region` belongs to, or `None` when this catalog publishes no
/// cross-region profile there.
#[must_use]
pub fn inference_profile_geography(region: &str) -> Option<&'static InferenceProfileGeography> {
    INFERENCE_PROFILE_GEOGRAPHIES
        .iter()
        .find(|geography| geography.regions.contains(&region))
}

/// The `description` a published profile reports: the model it routes to, and the
/// geography it routes within.
///
/// `InferenceProfileDescription` is modelled (len 1..200, `^([0-9a-zA-Z:.][ _-]?)+$`) and
/// a published profile carries one, so it is stated for every row by ONE rule rather than
/// written out per profile: what a cross-region profile IS, is that pair.
#[must_use]
pub fn system_defined_inference_profile_description(
    geography: &InferenceProfileGeography,
    model: &FoundationModel,
) -> String {
    format!(
        "Routes requests to {} in {} regions",
        cross_region_model_brand(model),
        geography.label
    )
}

/// Every system-defined inference profile Amazon Bedrock publishes in `region`, as the
/// `InferenceProfileSummary` a listing carries, in `inferenceProfileName` order — the
/// order a listing reports them in, because a published profile is created by no call and
/// so has nothing else to be ordered by.
///
/// A profile is published in the regions of its own geography, so a region belonging to no
/// geography this catalog carries has none. Each one reports one model per region of that
/// geography — the base model it routes invocations to there — and the instants a resource
/// nobody created has: the day the model it routes to launched
/// ([`FoundationModel::start_of_life_time`]), which is when the profile for it came into
/// being, for both `createdAt` and `updatedAt`, because nothing has updated it since.
#[must_use]
pub fn system_defined_inference_profiles(region: &str) -> Vec<InferenceProfileSummary> {
    let Some(geography) = inference_profile_geography(region) else {
        return Vec::new();
    };
    let mut profiles: Vec<InferenceProfileSummary> = CROSS_REGION_MODELS
        .iter()
        .filter_map(|model_id| catalogued_model(model_id))
        .map(|model| {
            let id = format!("{}.{}", geography.prefix, model.model_id);
            let published_at = published_instant(model.start_of_life_time);
            InferenceProfileSummary {
                inference_profile_name: format!(
                    "{} {}",
                    geography.label,
                    cross_region_model_brand(model)
                ),
                description: Some(system_defined_inference_profile_description(
                    geography, model,
                )),
                created_at: published_at,
                updated_at: published_at,
                inference_profile_arn: system_defined_inference_profile_arn(region, &id),
                models: geography
                    .regions
                    .iter()
                    .map(|routed| InferenceProfileModel {
                        model_arn: Some(foundation_model_arn(routed, model.model_id)),
                    })
                    .collect(),
                inference_profile_id: id,
                status: INFERENCE_PROFILE_ACTIVE.to_string(),
                r#type: INFERENCE_PROFILE_TYPE_SYSTEM_DEFINED.to_string(),
            }
        })
        .collect();
    profiles.sort_by(|left, right| {
        left.inference_profile_name
            .cmp(&right.inference_profile_name)
    });
    profiles
}
