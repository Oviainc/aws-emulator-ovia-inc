// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the STYLE RELEASE `GetStyleDescriptor` serves: the
//! parts of the document that state which release it is and how a renderer is
//! told to draw it.
//!
//! Each rule is asserted over a SET of inputs — every published style and
//! colour scheme, a table of request option sets, and every layer of the
//! published catalogue — rather than over one request:
//!
//! * the release version is ONE string, carried whole: `metadata.version` is
//!   the release's own version including any pre-release qualifier it carries,
//!   and the document's `id` is the style's scope followed by that same string;
//! * the virtual variants a style can be rendered as are published content of
//!   the style, so the block that declares them does not change with the
//!   request that read it;
//! * a layer's `filter` is an `all`-rooted list of complete clause
//!   expressions: a `match` over an attribute with its value list and
//!   true/false arms, and an `==` against `geometry-type` naming the geometry
//!   the layer's type draws;
//! * a layer declares the layout its type needs — a line layer states how its
//!   segments are joined, a fill states no layout at all.

use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_geomaps_mapsservice::state::style_layers::LayerRole;
use service_com_amazonaws_geomaps_mapsservice::state::{ColorSchemeKind, MapStyleKind};

use crate::map_assets::{assert_ok, body_text, Emulator};

/// The request option sets every rule below is asserted over: no options at
/// all, and sets drawn from the knobs `GetStyleDescriptor` models (terrain,
/// traffic, contour density, travel modes, buildings, point-of-interest
/// density and categories, political view), so a rule that held only for a
/// bare request is caught here.
const OPTION_SETS: &[&str] = &[
    "",
    "terrain=Terrain3D",
    "terrain=Hillshade&contour-density=High",
    "traffic=All&travel-modes=Truck&travel-modes=Transit",
    "buildings=Buildings3D&poi-density=VeryDense&poi-categories=FoodAndDrink",
    "political-view=IND&poi-density=Off",
];

/// One case per published style, colour scheme and option set: the case name
/// and the descriptor path that addresses it.
fn release_cases() -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            for options in OPTION_SETS {
                rows.push((
                    format!("{}/{}?{options}", style.as_wire(), scheme.as_wire()),
                    format!(
                        "/v2/styles/{}/descriptor?color-scheme={}&{options}",
                        style.as_wire(),
                        scheme.as_wire()
                    ),
                ));
            }
        }
    }
    rows
}

async fn document(emulator: &Emulator, case: &str, path: &str) -> serde_json::Value {
    let response = emulator.get(path).await;
    assert_ok(case, &response);
    serde_json::from_slice(response.body()).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: a style descriptor must be JSON, got {error} for {}",
            body_text(&response)
        )
    })
}

/// The release version is one string and is carried whole.
///
/// `metadata.version` is a semantic version — a three-part numeric core and,
/// while the release is a pre-release of that core, a qualifier after it — and
/// the document's `id` ends with that whole string, qualifier included. A
/// document that reported the core alone would claim the identity of a
/// different release of the same core.
#[tokio::test]
async fn the_release_version_is_carried_whole_into_the_identity() {
    let emulator = Emulator::new();
    let mut versions: BTreeSet<String> = BTreeSet::new();

    for (case, path) in release_cases() {
        let document = document(&emulator, &case, &path).await;
        let version = document["metadata"]["version"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the metadata must report the release version, got {}",
                    document["metadata"]
                )
            })
            .to_string();
        let id = document["id"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the document must report its id, got {}",
                    document["id"]
                )
            })
            .to_string();

        // The version's core is three dot-separated numbers; everything after
        // the first '-' is the pre-release qualifier, which is part of the
        // version and not a separate member.
        let (core, qualifier) = match version.split_once('-') {
            Some((core, qualifier)) => (core, Some(qualifier)),
            None => (version.as_str(), None),
        };
        let parts: Vec<&str> = core.split('.').collect();
        assert_eq!(
            parts.len(),
            3,
            "case {case:?}: expected a three-part version core, got {core:?} in {version:?}"
        );
        for part in &parts {
            assert!(
                !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()),
                "case {case:?}: expected a numeric version part, got {part:?} in {version:?}"
            );
        }
        if let Some(qualifier) = qualifier {
            assert!(
                !qualifier.is_empty(),
                "case {case:?}: a version's qualifier must not be empty, got {version:?}"
            );
            // The whole version, qualifier and all, is what the id ends with:
            // the core alone is a DIFFERENT release of this style.
            assert!(
                !id.ends_with(&format!("-{core}")),
                "case {case:?}: the id must not drop the qualifier {qualifier:?}, got {id:?}"
            );
        }

        let scope = id.strip_suffix(&format!("-{version}")).unwrap_or_else(|| {
            panic!(
                "case {case:?}: the id must end with the release version {version:?}, got {id:?}"
            )
        });
        // The scope the id is built on is the canonical name's, lowercased, so
        // the two members agree about which style this release is of.
        let name = document["name"].as_str().unwrap_or_default();
        assert!(
            name.to_lowercase().ends_with(scope),
            "case {case:?}: the id's scope {scope:?} must be the canonical name's, got {name:?}"
        );
        versions.insert(version);
    }
    assert_eq!(
        versions.len(),
        1,
        "every style of one catalog release reports that release's version, got {versions:?}"
    );
}

/// The virtual-variant block is published content of the style: it states the
/// variants the style CAN be rendered as, so every request against one style
/// reads the same block, whichever variant it selected.
#[tokio::test]
async fn the_declared_variants_do_not_depend_on_the_request() {
    let emulator = Emulator::new();
    // Case name -> the block it read, keyed on the block so a request that
    // changed it shows up as a second entry.
    let mut blocks: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for (case, path) in release_cases() {
        let document = document(&emulator, &case, &path).await;
        let block = &document["metadata"]["aws:virtual-variant"];
        let entries = block.as_array().unwrap_or_else(|| {
            panic!("case {case:?}: the variant block is part of the document, got {block}")
        });
        assert!(
            !entries.is_empty(),
            "case {case:?}: the block must declare the style's variants, got {block}"
        );
        for entry in entries {
            let key = entry["key"].as_str().unwrap_or_default();
            assert!(
                !key.is_empty(),
                "case {case:?}: a variant entry names the knob it varies, got {entry}"
            );
            let variants = entry["variants"].as_array().unwrap_or_else(|| {
                panic!("case {case:?}: entry {key:?} must list its variants, got {entry}")
            });
            assert!(
                !variants.is_empty(),
                "case {case:?}: entry {key:?} must declare at least one variant, got {entry}"
            );
            for variant in variants {
                let id = variant["id"].as_str().unwrap_or_default();
                assert!(
                    !id.is_empty(),
                    "case {case:?}: a declared variant is identified by its id, got {variant}"
                );
            }
        }
        blocks
            .entry(block.to_string())
            .or_default()
            .push(case.clone());
    }
    assert_eq!(
        blocks.len(),
        1,
        "the declared variants are the style's own, not the request's, got {:?}",
        blocks.keys().collect::<Vec<_>>()
    );
}

/// Every layer's filter is an `all`-rooted list of complete clause
/// expressions, and every clause names the geometry or the attribute it reads
/// as an expression rather than as a bare string: that is the form a renderer
/// evaluates, and a collapsed filter selects the wrong features.
#[tokio::test]
async fn every_layer_filters_through_an_all_rooted_clause_list() {
    let emulator = Emulator::new();
    // The geometry a layer of each MapLibre type draws, so the clause is
    // checked against the layer's own type rather than against a layer id.
    let geometry_of = |layer_type: &str| match layer_type {
        "fill" | "fill-extrusion" => Some("Polygon"),
        "line" => Some("LineString"),
        "symbol" => Some("Point"),
        _ => None,
    };

    for (case, path) in release_cases() {
        let document = document(&emulator, &case, &path).await;
        let layers = document["layers"].as_array().cloned().unwrap_or_default();
        assert!(
            layers.len() > 1,
            "case {case:?}: the document must declare the published layers, got {}",
            layers.len()
        );
        for layer in &layers {
            let id = layer["id"].as_str().unwrap_or_default();
            let layer_type = layer["type"].as_str().unwrap_or_default();
            let Some(geometry) = geometry_of(layer_type) else {
                // A background or raster layer reads no features, so it
                // filters nothing.
                assert!(
                    layer["filter"].is_null(),
                    "case {case:?}: layer {id} draws no features and must not filter, got {}",
                    layer["filter"]
                );
                continue;
            };
            let clauses = layer["filter"].as_array().cloned().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: layer {id} must filter to the features it draws, got {}",
                    layer["filter"]
                )
            });
            assert_eq!(
                clauses.first().and_then(serde_json::Value::as_str),
                Some("all"),
                "case {case:?}: layer {id}'s filter must be all-rooted, got {}",
                layer["filter"]
            );
            assert!(
                clauses.len() >= 3,
                "case {case:?}: layer {id} must state a clause per condition it applies, got {}",
                layer["filter"]
            );
            // Clause 1: the attribute values the layer draws, as a `match`
            // over the attribute with its value list and its true/false arms.
            let selector = &clauses[1];
            assert_eq!(
                selector[0].as_str(),
                Some("match"),
                "case {case:?}: layer {id} must select its values with a match, got {selector}"
            );
            assert_eq!(
                selector[1][0].as_str(),
                Some("get"),
                "case {case:?}: layer {id}'s match must read an attribute expression, got {selector}"
            );
            assert!(
                selector[1][1].as_str().is_some_and(|name| !name.is_empty()),
                "case {case:?}: layer {id}'s match must name the attribute it reads, got {selector}"
            );
            let values = selector[2].as_array().cloned().unwrap_or_default();
            assert!(
                !values.is_empty(),
                "case {case:?}: layer {id}'s match must list the values it draws, got {selector}"
            );
            assert_eq!(
                (selector[3].as_bool(), selector[4].as_bool()),
                (Some(true), Some(false)),
                "case {case:?}: layer {id}'s match must carry true/false arms, got {selector}"
            );
            // Clause 2: the geometry this layer's type draws.
            assert_eq!(
                clauses[2],
                serde_json::json!(["==", ["geometry-type"], geometry]),
                "case {case:?}: layer {id} is a {layer_type} layer, so it must select {geometry}, got {}",
                clauses[2]
            );
            // No clause is a bare string: a renderer evaluates expressions.
            for (position, clause) in clauses.iter().enumerate().skip(1) {
                assert!(
                    clause.is_array(),
                    "case {case:?}: layer {id}'s clause {position} must be an expression, got {clause}"
                );
            }
        }
    }
}

/// A layer declares the layout its type needs: a line layer states how the
/// segments of one way are joined, a symbol layer states what it places, and a
/// layer type with nothing to lay out declares no layout block.
#[tokio::test]
async fn a_layer_declares_the_layout_its_type_needs() {
    let emulator = Emulator::new();
    // The layout property each layer type must declare, derived from the
    // MapLibre type rather than from a layer id, plus the types that declare
    // none.
    let required = |layer_type: &str| match layer_type {
        "line" => Some("line-join"),
        "symbol" => Some("text-field"),
        _ => None,
    };
    let mut joins: BTreeSet<String> = BTreeSet::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (case, path) in release_cases() {
        let document = document(&emulator, &case, &path).await;
        for layer in document["layers"].as_array().into_iter().flatten() {
            let id = layer["id"].as_str().unwrap_or_default();
            let layer_type = layer["type"].as_str().unwrap_or_default();
            seen.insert(layer_type.to_string());
            match required(layer_type) {
                Some(property) => {
                    assert!(
                        !layer["layout"][property].is_null(),
                        "case {case:?}: layer {id} is a {layer_type} layer, so its layout must state {property}, got {}",
                        layer["layout"]
                    );
                    if property == "line-join" {
                        let join = layer["layout"]["line-join"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        assert!(
                            !join.is_empty(),
                            "case {case:?}: layer {id} must name how its segments join, got {}",
                            layer["layout"]
                        );
                        joins.insert(join);
                    }
                }
                None => assert!(
                    layer["layout"].is_null(),
                    "case {case:?}: layer {id} is a {layer_type} layer and lays nothing out, got {}",
                    layer["layout"]
                ),
            }
        }
    }
    // The catalogue really exercised both of the types that carry a layout.
    for layer_type in ["line", "symbol"] {
        assert!(
            seen.contains(layer_type),
            "the published catalogue must declare a {layer_type} layer, got {seen:?}"
        );
    }
    assert_eq!(
        joins.len(),
        1,
        "every published line layer joins its segments the same way, got {joins:?}"
    );
}

/// The roles the catalogue declares are the ones the filter rule above covers:
/// a role added to the catalogue draws one of the three geometries, so the
/// clause list stays exhaustive.
#[test]
fn every_catalogue_role_draws_one_of_the_filtered_geometries() {
    for role in [
        LayerRole::Area,
        LayerRole::Shading,
        LayerRole::Extrusion,
        LayerRole::Way,
        LayerRole::Casing,
        LayerRole::Shadow,
        LayerRole::Label,
        LayerRole::Icon,
    ] {
        let layer_type = role.layer_type();
        assert!(
            matches!(layer_type, "fill" | "fill-extrusion" | "line" | "symbol"),
            "case {role:?}: a published role must draw a filtered geometry, got {layer_type}"
        );
    }
}
