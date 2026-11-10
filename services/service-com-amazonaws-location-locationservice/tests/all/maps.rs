// SPDX-License-Identifier: CC0-1.0

//! The map control plane, end to end over the wire.
//!
//! Every call goes through the shared harness (`crate::harness`), so each one is a
//! separate HTTP request against one shared engine: a value that survives from the
//! create call to the describe call survived because it was PERSISTED, not because a
//! handler echoed its own input back.
//!
//! The tables span the classes the rules generalise over — the whole published map
//! style catalog and the data provider each style is sourced from, the optional
//! members of the create request (description, tags, political view, custom layers),
//! the shapes a rejected `Style` can take, the retired `PricingPlan` values, and the
//! request region the ARN is built from.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_location_locationservice::state;

// The one HTTP harness this crate's suites share, declared from `tests/all.rs`.
use crate::harness::{call, call_in_region, emulator, millis_of, Answer, ACCOUNT, REGION};

/// Every map style a map resource may be created with, and the data provider each
/// one's tiles come from — written out here rather than read from the crate.
///
/// The source is the `MapConfiguration$Style` member doc
/// (`smithy_query(query="doc",shape="MapConfiguration",member="Style")`), which
/// lists the styles under one heading per provider: "Valid Esri map styles", "Valid
/// HERE Technologies map styles", "Valid GrabMaps map styles" and "Valid Open Data
/// map styles". `VectorHereBerlin` is in the HERE group because that doc keeps it
/// there ("has been deprecated, but will continue to work in applications that use
/// it"). A catalog that silently loses or renames a style fails these tests instead
/// of shrinking with them.
const DOCUMENTED_STYLES: [(&str, &str); 18] = [
    ("VectorEsriDarkGrayCanvas", "Esri"),
    ("RasterEsriImagery", "Esri"),
    ("VectorEsriLightGrayCanvas", "Esri"),
    ("VectorEsriTopographic", "Esri"),
    ("VectorEsriStreets", "Esri"),
    ("VectorEsriNavigation", "Esri"),
    ("VectorHereContrast", "Here"),
    ("VectorHereBerlin", "Here"),
    ("VectorHereExplore", "Here"),
    ("VectorHereExploreTruck", "Here"),
    ("RasterHereExploreSatellite", "Here"),
    ("HybridHereExploreSatellite", "Here"),
    ("VectorGrabStandardLight", "Grab"),
    ("VectorGrabStandardDark", "Grab"),
    ("VectorOpenDataStandardLight", "Open Data"),
    ("VectorOpenDataStandardDark", "Open Data"),
    ("VectorOpenDataVisualizationLight", "Open Data"),
    ("VectorOpenDataVisualizationDark", "Open Data"),
];

/// The sentence a `Style` outside that catalog is refused with: the member name in
/// single quotes, the allowed values joined with ", ", then a period.
fn style_refusal() -> String {
    format!(
        "'Style' must be one of {}.",
        DOCUMENTED_STYLES
            .iter()
            .map(|(style, _)| *style)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// The sentence a retired `PricingPlan` is refused with.
const PRICING_PLAN_REFUSAL: &str = "'PricingPlan' must be RequestBasedUsage";

async fn create_map(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/maps/v0/maps", Some(body)).await
}

async fn describe_map(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "GET", &format!("/maps/v0/maps/{name}"), None).await
}

/// Assert one refused create: the status and code the model declares for a bad
/// member, the sentence, and the two members the model marks required beside
/// `message`.
fn assert_field_refusal(case: &str, refused: &Answer, member: &str, sentence: &str) {
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected a 400 refusal, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case {case:?}: expected error code ValidationException, got {:?} ({:?})",
        refused.error_code(),
        refused.body
    );
    assert_eq!(
        refused.message(),
        sentence,
        "case {case:?}: expected message {sentence:?}, got {:?}",
        refused.message()
    );
    let body = refused.json();
    assert_eq!(
        body.get("reason").and_then(Value::as_str),
        Some("FieldValidationFailed"),
        "case {case:?}: expected reason FieldValidationFailed, got {body}"
    );
    assert_eq!(
        body.get("fieldList")
            .and_then(Value::as_array)
            .map(|entries| entries
                .iter()
                .map(|entry| (
                    entry.get("name").and_then(Value::as_str).unwrap_or_default(),
                    entry
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                ))
                .collect::<Vec<_>>()),
        Some(vec![(member, sentence)]),
        "case {case:?}: fieldList must name the offending member and repeat the sentence, got {body}"
    );
}

/// The tags a response reports, sorted so a comparison does not depend on the
/// JSON object's order.
fn reported_tags(body: &Value) -> Vec<(String, String)> {
    let mut tags: Vec<(String, String)> = body
        .get("Tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default();
    tags.sort();
    tags
}

// ---------------------------------------------------------------------------
// CreateMap / DescribeMap
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_services_style_catalog_is_the_published_one() {
    // A style dropped from the crate's catalog, renamed, or attributed to another
    // provider shows up here rather than in a client's failed create.
    let catalog: Vec<(&str, &str)> = state::MAP_STYLES
        .iter()
        .map(|entry| (entry.style, entry.data_source))
        .collect();
    assert_eq!(
        catalog,
        DOCUMENTED_STYLES.to_vec(),
        "the service's style catalog must be the published one, got {catalog:?}"
    );
}

#[tokio::test]
async fn every_published_style_creates_a_map_that_reads_back_with_its_provider() {
    // One row per published style, including the styles the issue report did not
    // name: the whole domain of the rule, not a sample of it.
    for (index, (style, data_source)) in DOCUMENTED_STYLES.iter().enumerate() {
        let router = emulator();
        // The name differs per row and exercises the punctuation a `ResourceName`
        // admits (`^[-._\w]+$`).
        let name = format!("map.{index}_{style}");
        let created = create_map(
            &router,
            json!({ "MapName": name, "Configuration": { "Style": style } }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {style:?}: CreateMap must succeed, got {} {:?}",
            created.status, created.body
        );
        let created = created.json();
        let expected_arn = format!("arn:aws:geo:{REGION}:{ACCOUNT}:map/{name}");
        assert_eq!(
            created.get("MapName").and_then(Value::as_str),
            Some(name.as_str()),
            "case {style:?}: CreateMap must answer the created name, got {created}"
        );
        assert_eq!(
            created.get("MapArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {style:?}: expected MapArn {expected_arn}, got {created}"
        );
        let create_time = millis_of(&created, "CreateTime");

        let described = describe_map(&router, &name).await;
        assert_eq!(
            described.status, 200,
            "case {style:?}: DescribeMap must succeed, got {} {:?}",
            described.status, described.body
        );
        let described = described.json();
        assert_eq!(
            described.get("MapName").and_then(Value::as_str),
            Some(name.as_str()),
            "case {style:?}: DescribeMap must report the stored name, got {described}"
        );
        assert_eq!(
            described.get("MapArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {style:?}: DescribeMap must report the stored ARN, got {described}"
        );
        assert_eq!(
            described
                .get("Configuration")
                .and_then(|configuration| configuration.get("Style"))
                .and_then(Value::as_str),
            Some(*style),
            "case {style:?}: DescribeMap must report the stored Style, got {described}"
        );
        assert_eq!(
            described.get("DataSource").and_then(Value::as_str),
            Some(*data_source),
            "case {style:?}: expected DataSource {data_source:?}, got {described}"
        );
        // Every optional member of the request was absent, so what the read reports
        // for them comes from the service: `Description` is modelled required and
        // `len 0..1000`, so it reads back as the empty string, and the tag map is
        // projected as an empty map rather than omitted.
        assert_eq!(
            described.get("Description").and_then(Value::as_str),
            Some(""),
            "case {style:?}: an absent Description must read back empty, got {described}"
        );
        assert_eq!(
            reported_tags(&described),
            Vec::new(),
            "case {style:?}: an untagged map must report an empty Tags map, got {described}"
        );
        // `DescribeMapResponse$PricingPlan` is deprecated: "Always returns
        // RequestBasedUsage".
        assert_eq!(
            described.get("PricingPlan").and_then(Value::as_str),
            Some("RequestBasedUsage"),
            "case {style:?}: PricingPlan must always be RequestBasedUsage, got {described}"
        );
        assert_eq!(
            millis_of(&described, "CreateTime"),
            create_time,
            "case {style:?}: DescribeMap must report the CreateTime the create stored, got {described}"
        );
        assert_eq!(
            millis_of(&described, "UpdateTime"),
            create_time,
            "case {style:?}: a never-updated map's UpdateTime must equal its CreateTime, got {described}"
        );
    }
}

/// One row of the optional-member table: everything a create request may carry
/// beside the style, and what the read must report for it.
struct OptionalMembersCase {
    case: &'static str,
    request: fn() -> Value,
    description: &'static str,
    tags: &'static [(&'static str, &'static str)],
    political_view: Option<&'static str>,
    custom_layers: Option<&'static [&'static str]>,
}

/// The optional-member table. The rows differ along which optional members the
/// request carries and the sizes they take: none at all, a description and tags, a
/// political view (`CountryCode3`, pattern `^[A-Z]{3}$`), and custom layers (the
/// `CustomLayerList` is `@length max 10`, so the last row carries that maximum).
fn optional_member_cases() -> Vec<OptionalMembersCase> {
    vec![
        OptionalMembersCase {
            case: "nothing optional",
            request: || json!({ "Configuration": { "Style": "VectorEsriStreets" } }),
            description: "",
            tags: &[],
            political_view: None,
            custom_layers: None,
        },
        OptionalMembersCase {
            case: "description and tags",
            request: || {
                json!({
                    "Configuration": { "Style": "VectorHereExplore" },
                    "Description": "the delivery fleet basemap",
                    "Tags": { "team": "fleet", "env": "staging", "cost-center": "42" },
                })
            },
            description: "the delivery fleet basemap",
            tags: &[
                ("cost-center", "42"),
                ("env", "staging"),
                ("team", "fleet"),
            ],
            political_view: None,
            custom_layers: None,
        },
        OptionalMembersCase {
            case: "political view",
            request: || {
                json!({
                    "Configuration": { "Style": "VectorEsriNavigation", "PoliticalView": "IND" },
                    // Deprecated, and the only value it still admits.
                    "PricingPlan": "RequestBasedUsage",
                    "Description": "",
                })
            },
            description: "",
            tags: &[],
            political_view: Some("IND"),
            custom_layers: None,
        },
        OptionalMembersCase {
            case: "the largest CustomLayers list the model admits",
            request: || {
                json!({
                    "Configuration": {
                        "Style": "VectorEsriNavigation",
                        // `CustomLayerList` is `@length max 10`: the published bound
                        // itself. `POI` is the layer the member doc names.
                        "CustomLayers": [
                            "POI", "layer_2", "layer_3", "layer_4", "layer_5",
                            "layer_6", "layer_7", "layer_8", "layer_9", "layer.10",
                        ],
                    },
                    "Tags": { "owner": "maps" },
                })
            },
            description: "",
            tags: &[("owner", "maps")],
            political_view: None,
            custom_layers: Some(&[
                "POI", "layer_2", "layer_3", "layer_4", "layer_5", "layer_6", "layer_7",
                "layer_8", "layer_9", "layer.10",
            ]),
        },
    ]
}

#[tokio::test]
async fn a_map_reads_back_the_optional_members_the_create_carried() {
    for (index, case) in optional_member_cases().into_iter().enumerate() {
        let router = emulator();
        let name = format!("optional-{index}");
        let mut body = (case.request)();
        body["MapName"] = json!(name);

        let created = create_map(&router, body).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateMap must succeed, got {} {:?}",
            case.case, created.status, created.body
        );

        let described = describe_map(&router, &name).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: DescribeMap must succeed, got {} {:?}",
            case.case, described.status, described.body
        );
        let described = described.json();
        assert_eq!(
            described.get("Description").and_then(Value::as_str),
            Some(case.description),
            "case {:?}: expected Description {:?}, got {described}",
            case.case,
            case.description
        );
        let expected_tags: Vec<(String, String)> = case
            .tags
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        assert_eq!(
            reported_tags(&described),
            expected_tags,
            "case {:?}: expected Tags {expected_tags:?}, got {described}",
            case.case
        );
        let configuration = described
            .get("Configuration")
            .unwrap_or_else(|| panic!("case {:?}: Configuration must be present", case.case));
        assert_eq!(
            configuration.get("PoliticalView").and_then(Value::as_str),
            case.political_view,
            "case {:?}: expected PoliticalView {:?}, got {configuration}",
            case.case,
            case.political_view
        );
        let layers = configuration
            .get("CustomLayers")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| entry.as_str().unwrap_or_default().to_string())
                    .collect::<Vec<_>>()
            });
        let expected_layers = case.custom_layers.map(|layers| {
            layers
                .iter()
                .map(|layer| (*layer).to_string())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            layers, expected_layers,
            "case {:?}: expected CustomLayers {expected_layers:?}, got {configuration}",
            case.case
        );
    }
}

/// Every `Style` the published catalog does not hold, exactly as a request could
/// spell it.
///
/// The rows differ along the shapes a rejected value takes: case variants of
/// published styles (the catalog is matched as the doc spells it), a provider name
/// on its own, a style from the version 2 Maps API, a truncated style, and a
/// single character.
const REJECTED_STYLES: &[&str] = &[
    "vectoresristreets",
    "VECTORESRISTREETS",
    "VectorESRIStreets",
    "Esri",
    "Standard",
    "VectorEsri",
    "RasterEsriImagery2",
    "x",
];

#[tokio::test]
async fn a_style_outside_the_published_catalog_is_refused_and_persists_nothing() {
    let sentence = style_refusal();
    for (index, style) in REJECTED_STYLES.iter().enumerate() {
        let router = emulator();
        let name = format!("refused-{index}");
        let refused = create_map(
            &router,
            json!({ "MapName": name, "Configuration": { "Style": style } }),
        )
        .await;

        // The sentence is the same for every rejected value, which is what proves
        // the refusal states the catalog rather than echoing what was sent.
        assert_field_refusal(style, &refused, "Style", &sentence);

        // A refused create stores nothing: the read answers 404, not a resource.
        let described = describe_map(&router, &name).await;
        assert_eq!(
            described.status, 404,
            "case {style:?}: a refused create must persist nothing, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.error_code(),
            "ResourceNotFoundException",
            "case {style:?}: expected ResourceNotFoundException, got {:?} ({:?})",
            described.error_code(),
            described.body
        );
    }
}

#[tokio::test]
async fn a_retired_pricing_plan_is_refused_and_the_one_it_still_admits_is_not() {
    // `PricingPlan` doc: "No longer used. If included, the only allowed value is
    // RequestBasedUsage." The two rows are the enum's other members
    // (`smithy_query shape=PricingPlan`).
    for plan in ["MobileAssetManagement", "MobileAssetTracking"] {
        let router = emulator();
        let refused = create_map(
            &router,
            json!({
                "MapName": "retired",
                "Configuration": { "Style": "VectorEsriStreets" },
                "PricingPlan": plan,
            }),
        )
        .await;
        assert_field_refusal(plan, &refused, "PricingPlan", PRICING_PLAN_REFUSAL);
        assert_eq!(
            describe_map(&router, "retired").await.status,
            404,
            "case {plan:?}: a refused create must persist nothing"
        );
    }

    // Present and allowed, and absent, are both accepted.
    for plan in [Some("RequestBasedUsage"), None] {
        let router = emulator();
        let mut body = json!({
            "MapName": "allowed",
            "Configuration": { "Style": "VectorGrabStandardDark" },
        });
        if let Some(plan) = plan {
            body["PricingPlan"] = json!(plan);
        }
        let created = create_map(&router, body).await;
        assert_eq!(
            created.status, 200,
            "case {plan:?}: the allowed PricingPlan must be accepted, got {} {:?}",
            created.status, created.body
        );
    }
}

#[tokio::test]
async fn map_arn_carries_the_region_the_request_is_addressed_to() {
    // One ARN per request region, so the region segment is read from the request
    // rather than baked in.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let router = emulator();
        let body = bytes::Bytes::from(
            serde_json::to_vec(&json!({
                "MapName": "regional",
                "Configuration": { "Style": "VectorOpenDataStandardLight" },
            }))
            .expect("body serializes"),
        );
        let created = call_in_region(&router, "POST", "/maps/v0/maps", body, region).await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: CreateMap must succeed, got {} {:?}",
            created.status, created.body
        );
        let expected = format!("arn:aws:geo:{region}:{ACCOUNT}:map/regional");
        assert_eq!(
            created.json().get("MapArn").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {region:?}: expected MapArn {expected}, got {:?}",
            created.body
        );
    }
}

#[tokio::test]
async fn recreating_a_map_is_a_conflict_and_leaves_the_stored_one_alone() {
    let router = emulator();
    let first = create_map(
        &router,
        json!({
            "MapName": "twice",
            "Configuration": { "Style": "VectorEsriStreets" },
            "Description": "the stored one",
        }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "the first create must succeed, got {} {:?}",
        first.status, first.body
    );
    let stored_create_time = millis_of(&first.json(), "CreateTime");

    // A second create for the same name, with different members, must not
    // overwrite anything.
    for style in ["VectorHereContrast", "VectorGrabStandardLight"] {
        let again = create_map(
            &router,
            json!({
                "MapName": "twice",
                "Configuration": { "Style": style },
                "Description": "the overwrite that must not happen",
            }),
        )
        .await;
        assert_eq!(
            again.status, 409,
            "case {style:?}: re-creating a name must conflict, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ConflictException",
            "case {style:?}: expected ConflictException, got {:?} ({:?})",
            again.error_code(),
            again.body
        );
        assert_eq!(
            again.message(),
            "Map already exists: twice.",
            "case {style:?}: expected the conflict to name the resource and the requested name, got {:?}",
            again.message()
        );
    }

    let described = describe_map(&router, "twice").await.json();
    assert_eq!(
        described
            .get("Configuration")
            .and_then(|configuration| configuration.get("Style"))
            .and_then(Value::as_str),
        Some("VectorEsriStreets"),
        "the conflicting creates must not change the stored Style, got {described}"
    );
    assert_eq!(
        described.get("DataSource").and_then(Value::as_str),
        Some("Esri"),
        "the conflicting creates must not change the stored DataSource, got {described}"
    );
    assert_eq!(
        described.get("Description").and_then(Value::as_str),
        Some("the stored one"),
        "the conflicting creates must not change the stored Description, got {described}"
    );
    assert_eq!(
        millis_of(&described, "CreateTime"),
        stored_create_time,
        "the conflicting creates must not change the stored CreateTime, got {described}"
    );
}

#[tokio::test]
async fn describing_a_map_that_was_never_created_is_not_found() {
    let router = emulator();
    // One map exists; the names below satisfy `^[-._\w]+$` but were never created,
    // including case variants of the one that was: the lookup is by exact name.
    let created = create_map(
        &router,
        json!({ "MapName": "Exists", "Configuration": { "Style": "VectorEsriTopographic" } }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the positive control must be created, got {} {:?}",
        created.status, created.body
    );

    for name in ["missing", "exists", "EXISTS", "Exists.2", "x"] {
        let described = describe_map(&router, name).await;
        assert_eq!(
            described.status, 404,
            "case {name:?}: an unknown map must be 404, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.error_code(),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {:?} ({:?})",
            described.error_code(),
            described.body
        );
        let expected = format!("Map not found: {name}.");
        assert_eq!(
            described.message(),
            expected,
            "case {name:?}: expected message {expected:?}, got {:?}",
            described.message()
        );
    }
}

#[tokio::test]
async fn a_read_does_not_change_what_a_later_reader_sees() {
    let router = emulator();
    create_map(
        &router,
        json!({
            "MapName": "stable",
            "Configuration": { "Style": "HybridHereExploreSatellite" },
            "Tags": { "k": "v" },
        }),
    )
    .await;

    let first = describe_map(&router, "stable").await.json();
    for round in 1..=3 {
        let again = describe_map(&router, "stable").await;
        assert_eq!(
            again.status, 200,
            "round {round}: the read must keep succeeding, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.json(),
            first,
            "round {round}: DescribeMap must report the same resource every time, got {:?}",
            again.body
        );
    }
}

#[tokio::test]
async fn maps_are_stored_beside_one_another_rather_than_replacing_one_another() {
    let router = emulator();
    // Three maps created in one session, each with its own style: a create must not
    // evict or overwrite a map another create stored.
    let names = ["first", "second.map", "third_map"];
    let styles = [
        "VectorEsriDarkGrayCanvas",
        "VectorOpenDataVisualizationDark",
        "RasterHereExploreSatellite",
    ];
    for (name, style) in names.iter().zip(styles) {
        let created = create_map(
            &router,
            json!({ "MapName": name, "Configuration": { "Style": style } }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: CreateMap must succeed, got {} {:?}",
            created.status, created.body
        );
    }
    for (name, style) in names.iter().zip(styles) {
        let described = describe_map(&router, name).await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: every created map must still be readable, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described
                .json()
                .get("Configuration")
                .and_then(|configuration| configuration.get("Style"))
                .and_then(Value::as_str),
            Some(style),
            "case {name:?}: expected the Style it was created with, got {:?}",
            described.body
        );
    }
}
