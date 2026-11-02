// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the style descriptor `GetStyleDescriptor` publishes.
//!
//! The style document IS the client contract: a MapLibre client loads it and
//! resolves every other request it makes from it. The rules pinned here, each
//! over the whole set of published styles rather than one recorded request:
//!
//! * a published style reports its own identity — the canonical name of the
//!   style, its coverage, its colour scheme and its variant, the id that name
//!   plus the published version makes, the version itself and the publication
//!   instants, which do not move between reads;
//! * the render-wide blocks a client needs to draw a globe are present: the
//!   light, the sky and the projection;
//! * every URL the document hands back is ABSOLUTE and rooted at the endpoint
//!   the request addressed, so a client that does not resolve relative URLs
//!   still reaches this emulator;
//! * every source reports the tileset behind it: its type, its credit line and
//!   the zoom range that tileset really serves;
//! * the ground is a `background` layer painted in the style's own land colour,
//!   and every other layer is there because the tiles behind it carry it;
//! * a request's feature knobs change the document: the variant a terrain
//!   request is served by, the traffic source and its layers, and the
//!   categories a point-of-interest layer filters on.

use std::collections::{BTreeMap, BTreeSet};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_geomaps_mapsservice::state::{
    ColorSchemeKind, MapStyleKind, PaintValue, TileFormat, VariantKind,
};

use crate::map_assets::{
    assert_ok, body_text, expand_tile_template, expected_etag, header, path_of, Emulator, HOST,
};

/// Every (style, colour scheme) the catalog publishes, with the descriptor path
/// that addresses it.
fn published_styles() -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            rows.push((
                format!("{}/{}", style.as_wire(), scheme.as_wire()),
                format!(
                    "/v2/styles/{}/descriptor?color-scheme={}",
                    style.as_wire(),
                    scheme.as_wire()
                ),
            ));
        }
    }
    rows
}

/// The document served for one request, as JSON.
async fn style_document(emulator: &Emulator, case: &str, path: &str) -> serde_json::Value {
    let response = emulator.get(path).await;
    assert_ok(case, &response);
    serde_json::from_slice(response.body()).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: a style descriptor must be JSON, got {error} for {}",
            body_text(&response)
        )
    })
}

fn as_f64(value: &serde_json::Value, case: &str, what: &str) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: {what} must be a number, got {value}"))
}

/// A published style reports the identity of the document it is: the canonical
/// name of (style, coverage, colour scheme, variant), the id that name and the
/// published version make, the version, and the publication instants — which
/// are published content, so two reads report the same ones.
#[tokio::test]
async fn a_published_style_reports_its_own_identity() {
    let emulator = Emulator::new();
    let mut ids: BTreeMap<String, String> = BTreeMap::new();

    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            // One row per published style; the variant is the one the model
            // declares, which is also the sprite sheet each style is paired
            // with.
            let variant = VariantKind::Default;
            let case = format!("{}/{}", style.as_wire(), scheme.as_wire());
            let path = format!(
                "/v2/styles/{}/descriptor?color-scheme={}",
                style.as_wire(),
                scheme.as_wire()
            );
            let document = style_document(&emulator, &case, &path).await;

            let scope = format!(
                "{}-Global-{}-{}",
                style.as_wire(),
                scheme.as_wire(),
                variant.as_wire()
            );
            let expected_name = format!("AmazonLocation-{scope}");
            assert_eq!(
                document["name"].as_str(),
                Some(expected_name.as_str()),
                "case {case:?}: expected the canonical style name {expected_name}, got {}",
                document["name"]
            );

            let version = document["metadata"]["version"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: the metadata must report the published style version, got {}",
                        document["metadata"]
                    )
                })
                .to_string();
            assert!(
                !version.is_empty(),
                "case {case:?}: the published style version must not be empty"
            );
            let expected_id = format!("{}-{version}", scope.to_lowercase());
            assert_eq!(
                document["id"].as_str(),
                Some(expected_id.as_str()),
                "case {case:?}: expected the id {expected_id}, got {}",
                document["id"]
            );
            assert!(
                ids.insert(expected_id.clone(), case.clone()).is_none(),
                "case {case:?}: two published styles must not share the id {expected_id}"
            );

            for member in ["created", "modified"] {
                let text = document[member].as_str().unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: {member} must be a timestamp, got {}",
                        document[member]
                    )
                });
                DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
                    panic!("case {case:?}: {member} {text:?} must be RFC 3339, got {error}")
                });
            }

            // Published content: a second read reports the same instants and
            // the same id.
            let again = style_document(&emulator, &case, &path).await;
            for member in ["id", "created", "modified", "name"] {
                assert_eq!(
                    again[member], document[member],
                    "case {case:?}: {member} must not move between reads, got {} then {}",
                    document[member], again[member]
                );
            }
        }
    }
    assert_eq!(
        ids.len(),
        MapStyleKind::ALL.len() * ColorSchemeKind::ALL.len(),
        "every published style must answer with an id of its own, got {ids:?}"
    );
}

/// Every published style carries the render-wide blocks a client needs to draw
/// it: the light its 3D layers are lit by, the sky above the horizon and the
/// projection it is rendered in.
#[tokio::test]
async fn a_published_style_carries_its_light_sky_and_projection() {
    let emulator = Emulator::new();
    // The `sky` members a MapLibre style declares, split into the colours and
    // the blend factors, which are fractions.
    let sky_colors = ["sky-color", "horizon-color", "fog-color"];
    let sky_blends = [
        "sky-horizon-blend",
        "horizon-fog-blend",
        "fog-ground-blend",
        "atmosphere-blend",
    ];

    for (case, path) in published_styles() {
        let document = style_document(&emulator, &case, &path).await;

        let light = &document["light"];
        let anchor = light["anchor"].as_str().unwrap_or_default();
        assert!(
            matches!(anchor, "map" | "viewport"),
            "case {case:?}: the light anchor must be map or viewport, got {anchor:?}"
        );
        let intensity = as_f64(&light["intensity"], &case, "the light intensity");
        assert!(
            (0.0..=1.0).contains(&intensity),
            "case {case:?}: the light intensity must be a fraction, got {intensity}"
        );
        let position = light["position"].as_array().unwrap_or_else(|| {
            panic!(
                "case {case:?}: the light position must be [radial, azimuthal, polar], got {}",
                light["position"]
            )
        });
        assert_eq!(
            position.len(),
            3,
            "case {case:?}: the light position must carry three components, got {position:?}"
        );
        for component in position {
            as_f64(component, &case, "a light position component");
        }

        let sky = &document["sky"];
        for member in sky_colors {
            let color = sky[member].as_str().unwrap_or_default();
            assert!(
                color.starts_with("hsl(") || color.starts_with("hsla("),
                "case {case:?}: sky {member} must be a colour literal, got {color:?}"
            );
        }
        for member in sky_blends {
            let blend = as_f64(&sky[member], &case, &format!("sky {member}"));
            assert!(
                (0.0..=1.0).contains(&blend),
                "case {case:?}: sky {member} must be a fraction, got {blend}"
            );
        }

        let projection = document["projection"]["type"].as_str().unwrap_or_default();
        assert!(
            !projection.is_empty(),
            "case {case:?}: the document must state the projection it is rendered in, got {}",
            document["projection"]
        );
    }
}

/// Every URL a style document hands back — each source's tile template, the
/// sprite base and the glyph template — is absolute and rooted at the endpoint
/// the request addressed, and each source reports the tileset behind it: its
/// type, its credit line and the zoom range that tileset really serves (proved
/// by fetching the tile at the zoom it reports).
#[tokio::test]
async fn every_source_reports_its_tileset_and_an_absolute_template() {
    let emulator = Emulator::new();
    // The style-document source type each tileset format is declared as.
    let source_type = |format: TileFormat| match format {
        TileFormat::VectorMvt => "vector",
        TileFormat::RasterJpeg => "raster",
        TileFormat::RasterDemPng => "raster-dem",
    };
    let tilesets: BTreeMap<String, (TileFormat, String, u32, u32)> = {
        let catalog = emulator.catalog();
        let catalog = catalog.lock();
        catalog
            .tilesets
            .values()
            .map(|record| {
                (
                    record.id.clone(),
                    (
                        record.format,
                        record.attribution.clone(),
                        record.min_zoom,
                        record.max_zoom,
                    ),
                )
            })
            .collect()
    };

    for (case, path) in published_styles() {
        // Every knob that adds a source, so the rule is checked on the traffic
        // and terrain sources too and not only on the always-on ones.
        let path = format!("{path}&traffic=All&terrain=Terrain3D");
        let document = style_document(&emulator, &case, &path).await;
        let prefix = format!("https://{HOST}/v2/");

        let sources = document["sources"].as_object().unwrap_or_else(|| {
            panic!("case {case:?}: a style must declare its sources, got {}", document["sources"])
        });
        assert!(
            !sources.is_empty(),
            "case {case:?}: a style must declare at least one source"
        );
        for (name, source) in sources {
            let template = source["tiles"][0].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: source {name} must carry a tile template, got {source}")
            });
            assert!(
                template.starts_with(&prefix),
                "case {case:?}: source {name} must point at {prefix}, got {template}"
            );
            let tileset = template
                .rsplit("/v2/tiles/")
                .next()
                .and_then(|rest| rest.split('/').next())
                .unwrap_or_default()
                .to_string();
            let (format, attribution, min_zoom, max_zoom) = tilesets
                .get(&tileset)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: source {name} reads a tileset the catalog does not publish: {tileset}")
                })
                .clone();
            assert_eq!(
                source["type"].as_str(),
                Some(source_type(format)),
                "case {case:?}: source {name} must be declared as a {} source, got {}",
                source_type(format),
                source["type"]
            );
            assert_eq!(
                source["attribution"].as_str(),
                Some(attribution.as_str()),
                "case {case:?}: source {name} must report its credit line, got {}",
                source["attribution"]
            );
            assert_eq!(
                source["minzoom"].as_u64(),
                Some(u64::from(min_zoom)),
                "case {case:?}: source {name} must report the tileset's lowest zoom, got {}",
                source["minzoom"]
            );
            assert_eq!(
                source["maxzoom"].as_u64(),
                Some(u64::from(max_zoom)),
                "case {case:?}: source {name} must report the tileset's highest zoom, got {}",
                source["maxzoom"]
            );
            // The reported range is a claim about what GetTile serves: the
            // lowest and the highest zoom it names must both answer.
            for zoom in [min_zoom, max_zoom] {
                let tile = emulator
                    .get(&path_of(&expand_tile_template(template, zoom, 0, 0)))
                    .await;
                assert_ok(&format!("{case} source {name} z{zoom}"), &tile);
            }
        }

        for member in ["sprite", "glyphs"] {
            let template = document[member].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: the document must declare {member}, got {}", document[member])
            });
            assert!(
                template.starts_with(&prefix),
                "case {case:?}: {member} must be absolute under {prefix}, got {template}"
            );
        }
    }
}

/// The ground is painted as the document's `background` layer — a background
/// has no source geometry, which is what makes it the ground — with the paint
/// value the style publishes for it, and it is the first layer drawn.
#[tokio::test]
async fn the_ground_is_the_documents_background_layer() {
    let emulator = Emulator::new();
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            let case = format!("{}/{}", style.as_wire(), scheme.as_wire());
            // The published paint value, as the catalog states it: a colour for
            // a flat ground and an `interpolate` expression for one that
            // changes with zoom.
            let expected = {
                let catalog = emulator.catalog();
                let catalog = catalog.lock();
                match &catalog
                    .styles
                    .get(&(style, scheme))
                    .unwrap_or_else(|| panic!("case {case:?}: the catalog publishes this style"))
                    .background
                {
                    PaintValue::Flat(color) => serde_json::Value::String(color.css()),
                    PaintValue::ZoomRamp(stops) => {
                        let mut expression = vec![
                            serde_json::json!("interpolate"),
                            serde_json::json!(["linear"]),
                            serde_json::json!(["zoom"]),
                        ];
                        for (zoom, color) in stops {
                            expression.push(if zoom.fract() == 0.0 {
                                serde_json::json!(*zoom as i64)
                            } else {
                                serde_json::json!(zoom)
                            });
                            expression.push(serde_json::json!(color.css()));
                        }
                        serde_json::Value::Array(expression)
                    }
                }
            };
            let path = format!(
                "/v2/styles/{}/descriptor?color-scheme={}",
                style.as_wire(),
                scheme.as_wire()
            );
            let document = style_document(&emulator, &case, &path).await;
            let layers = document["layers"].as_array().unwrap_or_else(|| {
                panic!("case {case:?}: a style must declare layers, got {}", document["layers"])
            });

            let backgrounds: Vec<&serde_json::Value> = layers
                .iter()
                .filter(|layer| layer["type"] == "background")
                .collect();
            assert_eq!(
                backgrounds.len(),
                1,
                "case {case:?}: the ground is one background layer, got {backgrounds:?}"
            );
            let ground = backgrounds[0];
            assert_eq!(
                ground["id"], layers[0]["id"],
                "case {case:?}: the ground must be drawn first, got {}",
                layers[0]["id"]
            );
            let painted = &ground["paint"]["background-color"];
            assert_eq!(
                painted, &expected,
                "case {case:?}: the ground must carry the published paint value {expected}, got {painted}"
            );
            assert!(
                ground["source"].is_null() && ground["source-layer"].is_null(),
                "case {case:?}: a background layer reads no source, got {ground}"
            );
        }
    }
}

/// The virtual variants a published style can be rendered as are declared in
/// the document, keyed on the request knob that selects between them — the
/// block a client reads to discover what it may ask for, so it states what the
/// style has rather than what this request asked for.
#[tokio::test]
async fn a_terrain_request_records_the_virtual_variant_it_is_served_by() {
    let emulator = Emulator::new();
    // Row = a query. The terrain values are the model's own `Terrain` enum
    // (Hillshade | Terrain3D), plus requests that name no terrain at all.
    let cases = [
        "",
        "terrain=Terrain3D",
        "terrain=Hillshade",
        "terrain=Terrain3D&traffic=All&buildings=Buildings3D",
        "traffic=Congestion",
    ];

    let mut blocks: BTreeMap<String, String> = BTreeMap::new();
    for query in cases {
        let case = format!("Standard?{query}");
        let document = style_document(
            &emulator,
            &case,
            &format!("/v2/styles/Standard/descriptor?{query}"),
        )
        .await;
        let recorded = &document["metadata"]["aws:virtual-variant"];
        let entries = recorded.as_array().unwrap_or_else(|| {
            panic!(
                "case {case:?}: the document must always carry the variant entry, got {recorded}"
            )
        });
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: the entry names the knob the variants vary, got {recorded}"
        );
        assert_eq!(
            recorded[0]["key"].as_str(),
            Some("terrain"),
            "case {case:?}: the recorded variant must be keyed on terrain, got {recorded}"
        );
        let declared: Vec<&str> = recorded[0]["variants"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|variant| variant["id"].as_str())
            .collect();
        assert!(
            !declared.is_empty(),
            "case {case:?}: the style's terrain variants must be declared, got {recorded}"
        );
        blocks.insert(recorded.to_string(), case.clone());
    }
    assert_eq!(
        blocks.len(),
        1,
        "the declared variants are the style's own, not the request's, got {blocks:?}"
    );
}

/// Every layer in the document is there because the tiles behind it carry it,
/// and the knobs that switch a family on and off do exactly that: the traffic
/// source and its event layers, the point-of-interest family and the categories
/// it filters on.
#[tokio::test]
async fn the_layer_set_follows_the_tiles_and_the_requested_options() {
    let emulator = Emulator::new();
    // Row = (query, layer ids the document must carry, layer ids it must not).
    let cases: [(&str, &[&str], &[&str]); 6] = [
        // No knobs: the always-on basemap families, and nothing from a tileset
        // the request did not ask for.
        (
            "",
            &[
                "water_fill",
                "road_primary",
                "road_primary-casing",
                "rail_tunnel",
                "bridge_minor-shadow",
                "admin_country",
                "place_label_locality_city",
                "shield_us-state",
            ],
            &[
                "traffic_flow",
                "traffic_incidents",
                "contour_lines",
                "road_truck",
                "road_traffic_primary",
            ],
        ),
        // Congestion is flow events only; All adds the incident layers.
        (
            "traffic=Congestion",
            &["traffic_flow", "road_traffic_primary"],
            &["traffic_incidents", "traffic_incident_icons"],
        ),
        (
            "traffic=All",
            &["traffic_flow", "traffic_incidents", "traffic_incident_icons"],
            &[],
        ),
        // An additional feature is another layer in the tiles the template
        // fetches, so it is drawn.
        (
            "contour-density=Low&terrain=Hillshade",
            &["contour_lines", "hillshade"],
            &[],
        ),
        (
            "travel-modes=Transit&travel-modes=Truck",
            &["road_truck", "bridge_truck-casing", "shield_truck-hazmat"],
            &[],
        ),
        // Off hides points of interest altogether.
        (
            "poi-density=Off",
            &["water_fill", "road_primary"],
            &["poi_100_food_drink", "poi_600_shopping"],
        ),
    ];

    for (query, present, absent) in cases {
        let case = format!("Standard?{query}");
        let document = style_document(
            &emulator,
            &case,
            &format!("/v2/styles/Standard/descriptor?{query}"),
        )
        .await;
        let layers = document["layers"].as_array().cloned().unwrap_or_default();
        let ids: BTreeSet<String> = layers
            .iter()
            .filter_map(|layer| layer["id"].as_str().map(ToString::to_string))
            .collect();
        // A style document's layer ids are its layer handles, so no two layers
        // of one document may share one.
        assert_eq!(
            ids.len(),
            layers.len(),
            "case {case:?}: every layer must carry an id of its own, got {} layers for {} ids",
            layers.len(),
            ids.len()
        );
        for id in present {
            assert!(
                ids.contains(*id),
                "case {case:?}: expected a layer {id}, got {ids:?}"
            );
        }
        for id in absent {
            assert!(
                !ids.contains(*id),
                "case {case:?}: expected no layer {id}, got {ids:?}"
            );
        }

        // Every layer that reads a source must name one the document declares,
        // and every source-layer must be a layer that source's tiles carry.
        for layer in &layers {
            let (Some(source), Some(source_layer)) =
                (layer["source"].as_str(), layer["source-layer"].as_str())
            else {
                continue;
            };
            let template = document["sources"][source]["tiles"][0]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: layer {} reads an undeclared source {source}", layer["id"])
                });
            let tile = emulator
                .get(&path_of(&expand_tile_template(template, 4, 2, 3)))
                .await;
            assert_ok(&format!("{case} tile for {source}"), &tile);
            let decoded = <service_com_amazonaws_geomaps_mapsservice::state::vector_tile::Tile
                as prost::Message>::decode(tile.body().as_ref())
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: source {source} must serve a vector tile, got {error}")
                });
            let names: BTreeSet<&str> = decoded
                .layers
                .iter()
                .map(|layer| layer.name.as_str())
                .collect();
            assert!(
                names.contains(source_layer),
                "case {case:?}: layer {} reads source-layer {source_layer}, which the tiles of {source} do not carry: {names:?}",
                layer["id"]
            );
        }
    }
}

/// The requested point-of-interest categories are what the symbol layer filters
/// on, over the `category` attribute the tiles carry.
#[tokio::test]
async fn requested_poi_categories_filter_the_symbol_layer() {
    let emulator = Emulator::new();
    // Row = the categories asked for. Drawn from the model's `PoiCategory`
    // values, one, two and the length maximum of nine the member declares.
    let cases: [&[&str]; 4] = [
        &[],
        &["FoodAndDrink"],
        &["Shopping", "Transportation"],
        &[
            "FoodAndDrink",
            "Entertainment",
            "SightsAndMuseums",
            "Transportation",
            "Accommodations",
            "LeisureAndOutdoor",
            "Shopping",
            "BusinessAndServices",
            "FacilitiesAndBuildings",
        ],
    ];

    for categories in cases {
        let case = format!("poi-categories={categories:?}");
        let query: Vec<String> = categories
            .iter()
            .map(|category| format!("poi-categories={category}"))
            .collect();
        let document = style_document(
            &emulator,
            &case,
            &format!("/v2/styles/Standard/descriptor?{}", query.join("&")),
        )
        .await;
        let label = document["layers"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|layer| layer["id"] == "poi_100_food_drink")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the POI symbol layer must be drawn"));

        let rendered = label["filter"].to_string();
        for category in categories {
            assert!(
                rendered.contains(category),
                "case {case:?}: the symbol filter must name {category}, got {rendered}"
            );
        }
        assert_eq!(
            rendered.contains("category"),
            !categories.is_empty(),
            "case {case:?}: a filter on the category attribute is present only when categories were asked for, got {rendered}"
        );
        // The document records the published style's own facts, so the
        // categories asked for show up in the layer's filter and nowhere else.
        assert!(
            document["metadata"]["aws:poi-categories"].is_null(),
            "case {case:?}: the metadata does not echo the request, got {}",
            document["metadata"]
        );
    }
}

/// The descriptor's own headers: the media type the model documents the body
/// in, the catalog's cache directive, and an `ETag` that hashes the body
/// served.
#[tokio::test]
async fn the_descriptor_reports_the_catalog_headers() {
    let emulator = Emulator::new();
    let cache_control = {
        let catalog = emulator.catalog();
        let catalog = catalog.lock();
        catalog
            .styles
            .get(&(MapStyleKind::Standard, ColorSchemeKind::Light))
            .expect("the catalog publishes the Standard style")
            .cache_control
            .clone()
    };
    for (case, path) in published_styles() {
        let response = emulator.get(&path).await;
        assert_ok(&case, &response);
        // `GetStyleDescriptorResponse$Blob` documents the body as
        // application/json.
        assert_eq!(
            header(&response, "content-type"),
            "application/json",
            "case {case:?}: a style descriptor is a JSON document"
        );
        assert_eq!(
            header(&response, "cache-control"),
            cache_control,
            "case {case:?}: the cache directive must come from the catalog"
        );
        assert_eq!(
            header(&response, "etag"),
            expected_etag(response.body()),
            "case {case:?}: the ETag must hash the body served"
        );
    }
}
