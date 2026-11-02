// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PUBLISHED style document `GetStyleDescriptor`
//! serves: the rules that make the document the map rather than a sketch of
//! one.
//!
//! Each rule is asserted over a SET of inputs — every published style, every
//! source, every layer of the published catalogue, and a sweep of the colour
//! space — rather than over one request:
//!
//! * every colour the document states is written in the published HSL
//!   notation, and a colour that is not opaque is written as `hsla()` so its
//!   alpha survives into the document;
//! * a paint property whose published value changes with zoom is serialized as
//!   an `interpolate` EXPRESSION, which is a JSON array, not a string;
//! * every source's credit line names EVERY supplier of the data behind it,
//!   each linked to that supplier's own notice;
//! * the layer list is the published style's own: every catalogue entry whose
//!   tiles carry its tile layer is declared, with a unique id, the source it
//!   reads, the geometry it filters to and paint properties of its type;
//! * a casing outlines the way above it (darker and wider) and a shadow lifts a
//!   bridge off what it crosses (wider still and translucent);
//! * the virtual-variant entry is part of the document whether or not a variant
//!   is in effect, and the light block belongs to the style, not to the scheme.

use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_geomaps_mapsservice::state::boundaries::supported_views;
use service_com_amazonaws_geomaps_mapsservice::state::style_layers::{
    LayerGate, LayerRole, StyleLayerSpec, BASEMAP_LAYERS,
};
use service_com_amazonaws_geomaps_mapsservice::state::{
    attribution, ColorSchemeKind, Hsla, MapStyleKind, DATA_SUPPLIERS, OPAQUE,
};

use crate::map_assets::{assert_ok, body_text, Emulator};

/// One published style, as the case name and the descriptor path addressing it.
fn style_cases() -> Vec<(String, String)> {
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

/// A colour literal taken apart again: `(hue, saturation, lightness, alpha in
/// thousandths)`, or `None` when the text is not one of the two published
/// notations.
fn parse_css(text: &str) -> Option<(u16, u8, u8, u16)> {
    let (opaque, body) = match text.strip_prefix("hsla(") {
        Some(body) => (false, body),
        None => (true, text.strip_prefix("hsl(")?),
    };
    let body = body.strip_suffix(')')?;
    let parts: Vec<&str> = body.split(", ").collect();
    if parts.len() != if opaque { 3 } else { 4 } {
        return None;
    }
    let hue: u16 = parts[0].parse().ok()?;
    let saturation: u8 = parts[1].strip_suffix('%')?.parse().ok()?;
    let lightness: u8 = parts[2].strip_suffix('%')?.parse().ok()?;
    let alpha = if opaque {
        OPAQUE
    } else {
        let fraction: f64 = parts[3].parse().ok()?;
        (fraction * f64::from(OPAQUE)).round() as u16
    };
    Some((hue, saturation, lightness, alpha))
}

/// Every string the document uses as a colour, under the paint keys that take
/// one, paired with the layer it came from.
fn colour_literals(document: &serde_json::Value) -> Vec<(String, String)> {
    // The paint properties of the MapLibre style specification whose value is a
    // colour; every layer type this catalogue emits carries one of them.
    const COLOUR_KEYS: &[&str] = &[
        "background-color",
        "fill-color",
        "fill-extrusion-color",
        "line-color",
        "text-color",
        "text-halo-color",
    ];
    let mut found = Vec::new();
    for layer in document["layers"].as_array().into_iter().flatten() {
        let id = layer["id"].as_str().unwrap_or_default().to_string();
        for key in COLOUR_KEYS {
            if let Some(text) = layer["paint"][*key].as_str() {
                found.push((format!("{id}.{key}"), text.to_string()));
            }
        }
    }
    for member in ["sky-color", "horizon-color", "fog-color"] {
        if let Some(text) = document["sky"][member].as_str() {
            found.push((format!("sky.{member}"), text.to_string()));
        }
    }
    found
}

/// Every colour a published document states is written in the HSL notation the
/// published styles use, and only a colour that is not opaque is written as
/// `hsla()` — which is the notation that carries the alpha channel at all.
#[tokio::test]
async fn every_colour_is_a_published_hsl_literal() {
    let emulator = Emulator::new();
    for (case, path) in style_cases() {
        // Every knob that adds layers, so the rule is checked over the traffic,
        // terrain, contour, travel-mode and building families too.
        let path = format!(
            "{path}&traffic=All&terrain=Hillshade&contour-density=High\
             &travel-modes=Truck&travel-modes=Transit&buildings=Buildings3D&political-view=IND"
        );
        let document = document(&emulator, &case, &path).await;
        let literals = colour_literals(&document);
        // The ground and the three sky bands at the very least; a style with a
        // vector basemap paints every layer of the catalogue as well.
        assert!(
            literals.len() >= 4,
            "case {case:?}: a published document states its colours, got {} of them",
            literals.len()
        );
        for (where_, text) in literals {
            let parsed = parse_css(&text).unwrap_or_else(|| {
                panic!("case {case:?}: {where_} must be an hsl()/hsla() literal, got {text:?}")
            });
            let (hue, saturation, lightness, alpha) = parsed;
            assert!(
                hue < 360 && saturation <= 100 && lightness <= 100,
                "case {case:?}: {where_} must be in range, got {text:?}"
            );
            assert_eq!(
                text.starts_with("hsla("),
                alpha < OPAQUE,
                "case {case:?}: {where_} must use hsla() exactly when it is translucent, got {text:?}"
            );
        }
    }
}

/// A colour survives the notation: every value the catalogue can hold — across
/// the hue circle, both saturation bounds, both lightness bounds and a span of
/// alphas — is written so that it reads back unchanged, and an alpha below full
/// opacity is never lost.
#[test]
fn a_catalogue_colour_round_trips_through_its_literal() {
    // The published bounds of each component, swept rather than sampled: hue
    // over the whole circle, saturation and lightness over 0..=100 including
    // both ends, and the alphas a published style states (full opacity, the
    // translucent fog band, and the extremes).
    let alphas = [OPAQUE, 800, 500, 200, 125, 50, 0];
    let mut cases = 0_u32;
    for hue in (0..360).step_by(7) {
        for saturation in (0..=100).step_by(5) {
            for lightness in (0..=100).step_by(5) {
                for alpha in alphas {
                    let color = Hsla::new(hue, saturation, lightness).with_alpha(alpha);
                    let text = color.css();
                    let parsed = parse_css(&text).unwrap_or_else(|| {
                        panic!("case {color:?}: the literal {text:?} must be readable")
                    });
                    assert_eq!(
                        parsed,
                        (hue, saturation, lightness, alpha),
                        "case {color:?}: expected the literal to read back unchanged, got {text:?}"
                    );
                    assert_eq!(
                        color.is_opaque(),
                        alpha >= OPAQUE,
                        "case {color:?}: opacity must follow the alpha, got {text:?}"
                    );
                    cases += 1;
                }
            }
        }
    }
    assert!(
        cases > 10_000,
        "the sweep must cover the colour space, got {cases} cases"
    );
}

/// A colour is also a pixel: the conversion the rasterizers use stays inside
/// the byte range at every published bound, and an achromatic colour converts
/// to a grey whose level follows its lightness whatever hue it is written at.
#[test]
fn a_catalogue_colour_converts_to_a_pixel() {
    // Row = (hue written, lightness), over both ends of the lightness range and
    // the hue circle. A zero saturation shows no hue, so the pixel must not
    // depend on the one the literal states.
    for lightness in [0, 1, 12, 50, 87, 99, 100] {
        let mut greys = BTreeSet::new();
        for hue in [0, 37, 180, 228, 359] {
            let pixel = Hsla::new(hue, 0, lightness).rgba();
            assert_eq!(
                (pixel[0], pixel[1], pixel[2]),
                (pixel[0], pixel[0], pixel[0]),
                "case (hue {hue}, lightness {lightness}): a zero saturation is grey, got {pixel:?}"
            );
            greys.insert(pixel[0]);
        }
        assert_eq!(
            greys.len(),
            1,
            "case lightness {lightness}: the hue of an achromatic colour must not change the pixel, got {greys:?}"
        );
        let expected = (f64::from(lightness) / 100.0 * 255.0).round() as u8;
        assert_eq!(
            greys.iter().next_back().copied(),
            Some(expected),
            "case lightness {lightness}: expected the grey level {expected}, got {greys:?}"
        );
    }
    // A translucent colour keeps its alpha as a pixel too.
    for alpha in [OPAQUE, 800, 200, 0] {
        let pixel = Hsla::new(208, 90, 72).with_alpha(alpha).rgba();
        let expected = (f64::from(alpha) / f64::from(OPAQUE) * 255.0).round() as u8;
        assert_eq!(
            pixel[3], expected,
            "case alpha {alpha}: expected the pixel alpha {expected}, got {pixel:?}"
        );
    }
}

/// A published paint value that changes with zoom is served as an `interpolate`
/// EXPRESSION — a JSON array whose stops ascend — and one that does not is
/// served as a colour literal. A renderer handed a string where the style
/// declares an expression cannot vary the property at all.
#[tokio::test]
async fn a_zoom_dependent_paint_value_is_served_as_an_expression() {
    let emulator = Emulator::new();
    for (case, path) in style_cases() {
        let document = document(&emulator, &case, &path).await;
        let ground = document["layers"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|layer| layer["type"] == "background")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the ground is the background layer"));
        let painted = &ground["paint"]["background-color"];

        // What the catalogue publishes for this style decides which of the two
        // forms is correct; both are asserted, so neither form can be served
        // where the other belongs.
        let expression = painted.as_array();
        match expression {
            None => {
                let text = painted.as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: a flat ground is a colour literal, got {painted}")
                });
                assert!(
                    parse_css(text).is_some(),
                    "case {case:?}: a flat ground must be an hsl() literal, got {text:?}"
                );
            }
            Some(stops) => {
                assert_eq!(
                    stops.first().and_then(serde_json::Value::as_str),
                    Some("interpolate"),
                    "case {case:?}: a zoom-dependent paint value is an interpolate expression, got {painted}"
                );
                assert_eq!(
                    stops.get(1),
                    Some(&serde_json::json!(["linear"])),
                    "case {case:?}: the interpolation must name its curve, got {painted}"
                );
                assert_eq!(
                    stops.get(2),
                    Some(&serde_json::json!(["zoom"])),
                    "case {case:?}: the interpolation must be over zoom, got {painted}"
                );
                let pairs = &stops[3..];
                assert!(
                    pairs.len() >= 4 && pairs.len() % 2 == 0,
                    "case {case:?}: an interpolation needs at least two (zoom, colour) stops, got {painted}"
                );
                let mut previous = f64::MIN;
                for pair in pairs.chunks(2) {
                    let zoom = pair[0].as_f64().unwrap_or_else(|| {
                        panic!("case {case:?}: a stop's zoom must be a number, got {painted}")
                    });
                    assert!(
                        zoom > previous,
                        "case {case:?}: the stops must ascend, got {zoom} after {previous}"
                    );
                    previous = zoom;
                    let text = pair[1].as_str().unwrap_or_else(|| {
                        panic!("case {case:?}: a stop's value must be a colour, got {painted}")
                    });
                    assert!(
                        parse_css(text).is_some(),
                        "case {case:?}: a stop's colour must be an hsl() literal, got {text:?}"
                    );
                }
            }
        }
    }
}

/// Every source reports a credit line that names EVERY supplier of the data
/// behind it, each as a link to that supplier's own notice: a client renders the
/// line verbatim, so a supplier the line leaves out is a notice never shown.
#[tokio::test]
async fn every_source_credits_every_data_supplier() {
    let emulator = Emulator::new();
    // The whole supplier list, drawn from the catalogue rather than written out
    // here, so a supplier added to it is checked without this test changing.
    assert!(
        DATA_SUPPLIERS.len() >= 2,
        "the catalogue must name the service and its content suppliers, got {DATA_SUPPLIERS:?}"
    );
    for (case, path) in style_cases() {
        // Every source a request can add, so the traffic and terrain sources
        // are credited too and not only the always-on ones.
        let path = format!("{path}&traffic=All&terrain=Terrain3D");
        let document = document(&emulator, &case, &path).await;
        let sources = document["sources"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        assert!(
            !sources.is_empty(),
            "case {case:?}: a style must declare at least one source"
        );
        for (name, source) in &sources {
            let line = source["attribution"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: source {name} must report a credit line, got {source}")
            });
            for supplier in DATA_SUPPLIERS {
                let link = format!("<a href='{}'>{}</a>", supplier.notice_url, supplier.name);
                assert!(
                    line.contains(&link),
                    "case {case:?}: source {name} must credit {} with a link to its notice, got {line:?}",
                    supplier.name
                );
            }
            assert!(
                line.starts_with('\u{a9}'),
                "case {case:?}: source {name}'s credit line must open with the copyright sign, got {line:?}"
            );
            assert_eq!(
                line,
                attribution(),
                "case {case:?}: source {name} must report the catalogue's credit line"
            );
        }
    }
}

/// The layer list is the published style's own: every catalogue entry the
/// request leaves switched on is declared, each with an id no other layer
/// shares, the source and tile layer it reads, the geometry it filters to and
/// paint properties of its own type.
#[tokio::test]
async fn the_document_declares_the_published_layer_catalogue() {
    let emulator = Emulator::new();
    // The paint property each role must carry, derived from the role so a role
    // added to the catalogue is checked here too.
    let required_paint = |role: LayerRole| match role {
        LayerRole::Area | LayerRole::Shading => "fill-color",
        LayerRole::Extrusion => "fill-extrusion-color",
        LayerRole::Way | LayerRole::Casing | LayerRole::Shadow => "line-color",
        LayerRole::Label | LayerRole::Icon => "text-color",
    };

    // Every knob that switches a family on, so the whole catalogue is expected.
    let query = "traffic=All&terrain=Hillshade&contour-density=High\
                 &travel-modes=Truck&travel-modes=Transit&buildings=Buildings3D&political-view=IND";
    for style in [MapStyleKind::Standard, MapStyleKind::Monochrome] {
        for scheme in ColorSchemeKind::ALL {
            let case = format!("{}/{}?{query}", style.as_wire(), scheme.as_wire());
            let path = format!(
                "/v2/styles/{}/descriptor?color-scheme={}&{query}",
                style.as_wire(),
                scheme.as_wire()
            );
            let document = document(&emulator, &case, &path).await;
            let layers = document["layers"].as_array().cloned().unwrap_or_default();
            let by_id: BTreeMap<String, serde_json::Value> = layers
                .iter()
                .filter_map(|layer| {
                    layer["id"]
                        .as_str()
                        .map(|id| (id.to_string(), layer.clone()))
                })
                .collect();
            assert_eq!(
                by_id.len(),
                layers.len(),
                "case {case:?}: every layer must carry an id of its own, got {} layers for {} ids",
                layers.len(),
                by_id.len()
            );

            for spec in BASEMAP_LAYERS {
                let layer = by_id.get(spec.id).unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: the published layer {} must be declared, got {:?}",
                        spec.id,
                        by_id.keys().collect::<Vec<_>>()
                    )
                });
                assert_eq!(
                    layer["type"].as_str(),
                    Some(spec.role.layer_type()),
                    "case {case:?}: layer {} must be a {} layer, got {}",
                    spec.id,
                    spec.role.layer_type(),
                    layer["type"]
                );
                assert_eq!(
                    layer["source-layer"].as_str(),
                    Some(spec.source_layer),
                    "case {case:?}: layer {} must read {}, got {}",
                    spec.id,
                    spec.source_layer,
                    layer["source-layer"]
                );
                let source = layer["source"].as_str().unwrap_or_default();
                assert!(
                    document["sources"][source].is_object(),
                    "case {case:?}: layer {} reads an undeclared source {source:?}",
                    spec.id
                );
                assert!(
                    layer["filter"].is_array(),
                    "case {case:?}: layer {} must filter to the geometry it draws, got {}",
                    spec.id,
                    layer["filter"]
                );
                let key = required_paint(spec.role);
                assert!(
                    !layer["paint"][key].is_null(),
                    "case {case:?}: layer {} must carry {key}, got {}",
                    spec.id,
                    layer["paint"]
                );
                // A line layer's width is a zoom interpolation, and an
                // interpolation's stops must ascend.
                if key == "line-color" {
                    let stops = layer["paint"]["line-width"]
                        .as_array()
                        .cloned()
                        .unwrap_or_else(|| {
                            panic!(
                                "case {case:?}: layer {} must state its width as an interpolation, got {}",
                                spec.id, layer["paint"]["line-width"]
                            )
                        });
                    let mut previous = f64::MIN;
                    for pair in stops[3..].chunks(2) {
                        let zoom = pair[0].as_f64().unwrap_or_else(|| {
                            panic!("case {case:?}: layer {} has a non-numeric stop", spec.id)
                        });
                        assert!(
                            zoom > previous,
                            "case {case:?}: layer {} width stops must ascend, got {zoom} after {previous}",
                            spec.id
                        );
                        previous = zoom;
                        let width = pair[1].as_f64().unwrap_or_default();
                        assert!(
                            width > 0.0,
                            "case {case:?}: layer {} must be drawn wider than nothing, got {width}",
                            spec.id
                        );
                    }
                }
                assert_eq!(
                    layer["minzoom"].as_u64(),
                    (spec.min_zoom > 0).then_some(u64::from(spec.min_zoom)),
                    "case {case:?}: layer {} must come in at zoom {}, got {}",
                    spec.id,
                    spec.min_zoom,
                    layer["minzoom"]
                );
            }
        }
    }
}

/// A casing outlines the carriageway above it and a shadow lifts a bridge off
/// what it crosses: the casing is darker and wider than the way it outlines, and
/// the shadow is wider still and translucent.
#[tokio::test]
async fn a_casing_outlines_the_way_above_it() {
    let emulator = Emulator::new();
    // Row = (the way, its casing, its shadow or None) over the grades the
    // published network is drawn in, from the widest to the narrowest.
    let rows: [(&str, &str, Option<&str>); 5] = [
        (
            "bridge_motorway-trunk",
            "bridge_motorway-trunk-casing",
            Some("bridge_motorway-trunk-shadow"),
        ),
        ("road_primary", "road_primary-casing", None),
        ("road_tertiary", "road_tertiary-casing", None),
        (
            "bridge_service",
            "bridge_service-casing",
            Some("bridge_service-shadow"),
        ),
        ("tunnel_minor", "tunnel_minor-casing", None),
    ];
    let document = document(
        &emulator,
        "Standard/Light casings",
        "/v2/styles/Standard/descriptor?traffic=All",
    )
    .await;
    let by_id: BTreeMap<String, serde_json::Value> = document["layers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|layer| {
            layer["id"]
                .as_str()
                .map(|id| (id.to_string(), layer.clone()))
        })
        .collect();

    // The width a line layer comes in at: the first stop of its interpolation.
    let entry_width = |layer: &serde_json::Value, case: &str| {
        layer["paint"]["line-width"][4].as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: a line layer's width is a zoom interpolation, got {}",
                layer["paint"]["line-width"]
            )
        })
    };
    let colour = |layer: &serde_json::Value, case: &str| {
        let text = layer["paint"]["line-color"].as_str().unwrap_or_default();
        parse_css(text)
            .unwrap_or_else(|| panic!("case {case:?}: expected an hsl() literal, got {text:?}"))
    };

    for (way_id, casing_id, shadow_id) in rows {
        let way = by_id
            .get(way_id)
            .unwrap_or_else(|| panic!("case {way_id:?}: the way must be declared"));
        let casing = by_id
            .get(casing_id)
            .unwrap_or_else(|| panic!("case {casing_id:?}: the casing must be declared"));
        let (way_width, casing_width) = (entry_width(way, way_id), entry_width(casing, casing_id));
        assert!(
            casing_width > way_width,
            "case {casing_id:?}: expected a casing wider than {way_width}, got {casing_width}"
        );
        let (_, _, way_light, way_alpha) = colour(way, way_id);
        let (_, _, casing_light, casing_alpha) = colour(casing, casing_id);
        assert!(
            casing_light < way_light,
            "case {casing_id:?}: expected a casing darker than lightness {way_light}%, got {casing_light}%"
        );
        assert_eq!(
            (way_alpha, casing_alpha),
            (OPAQUE, OPAQUE),
            "case {casing_id:?}: a carriageway and its casing are opaque"
        );

        let Some(shadow_id) = shadow_id else {
            assert!(
                !by_id.contains_key(&format!("{way_id}-shadow")),
                "case {way_id:?}: only a bridge is lifted off what it crosses"
            );
            continue;
        };
        let shadow = by_id
            .get(shadow_id)
            .unwrap_or_else(|| panic!("case {shadow_id:?}: the shadow must be declared"));
        let shadow_width = entry_width(shadow, shadow_id);
        assert!(
            shadow_width > casing_width,
            "case {shadow_id:?}: expected a shadow wider than {casing_width}, got {shadow_width}"
        );
        let (_, _, _, shadow_alpha) = colour(shadow, shadow_id);
        assert!(
            shadow_alpha < OPAQUE,
            "case {shadow_id:?}: a shadow is translucent, got alpha {shadow_alpha}"
        );
    }
}

/// The variant block is part of every document and states what the published
/// style HAS, so it is the same whatever the request asked for; the light block
/// belongs to the published style rather than to the colour scheme.
#[tokio::test]
async fn the_document_always_reports_its_variants_and_its_light() {
    let emulator = Emulator::new();
    // Row = a query. The terrain values are the model's own `Terrain` enum, so
    // a request that selects a variant, one that selects the other and ones
    // that select none are all covered.
    let rows = [
        "",
        "traffic=All&buildings=Buildings3D",
        "terrain=Hillshade",
        "terrain=Terrain3D&poi-density=Off",
    ];
    let mut lights: BTreeSet<String> = BTreeSet::new();
    let mut advertised: BTreeSet<String> = BTreeSet::new();
    for query in rows {
        for (name, path) in style_cases() {
            let case = format!("{name}?{query}");
            let document = document(&emulator, &case, &format!("{path}&{query}")).await;
            let recorded = &document["metadata"]["aws:virtual-variant"];
            let entries = recorded.as_array().unwrap_or_else(|| {
                panic!("case {case:?}: the variant entry is part of the document, got {recorded}")
            });
            let ids: Vec<&str> = entries
                .iter()
                .flat_map(|entry| entry["variants"].as_array().into_iter().flatten())
                .filter_map(|variant| variant["id"].as_str())
                .collect();
            assert!(
                !ids.is_empty(),
                "case {case:?}: the block must state the variants the style has, got {recorded}"
            );
            for entry in entries {
                assert_eq!(
                    entry["key"].as_str(),
                    Some("terrain"),
                    "case {case:?}: a variant entry names the knob it varies, got {entry}"
                );
            }
            advertised.insert(recorded.to_string());
            lights.insert(document["light"].to_string());
        }
    }
    assert_eq!(
        advertised.len(),
        1,
        "the variants a style publishes do not depend on the request that read them, got {advertised:?}"
    );
    assert_eq!(
        lights.len(),
        1,
        "the light block belongs to the style, not to the scheme or the knobs, got {lights:?}"
    );
}

/// The requested political view is what the disputed-boundary layers draw. Such
/// a layer is part of every published document — a client switches views without
/// fetching another style — and the view in effect is what it selects: the
/// territories that view claims, on the attribute the tiles behind it carry, and
/// nothing at all when the request named no view.
#[tokio::test]
async fn the_requested_political_view_selects_the_boundary_alignment() {
    let emulator = Emulator::new();
    // The layers that draw an alignment only one view recognises.
    let disputed = [
        "admin_country-disputed",
        "admin_country-disputed-mask",
        "admin_region-disputed",
        "admin_region-disputed-mask",
    ];
    // Row = the view asked for, or None. Every view the catalogue has a
    // disputed territory for is a case, plus one request that names none; the
    // views come from `boundaries::DISPUTED_TERRITORIES`, so a view the model
    // adds is covered without this row list changing.
    let views: BTreeSet<&'static str> = supported_views().into_iter().collect();
    assert!(
        views.len() > 3,
        "the catalogue must have disputed territories for several views, got {views:?}"
    );
    let mut cases: Vec<Option<&str>> = vec![None];
    cases.extend(views.iter().map(|view| Some(*view)));

    for view in cases {
        let case = format!("Standard political-view={view:?}");
        let query = match view {
            Some(view) => format!("political-view={view}"),
            None => String::new(),
        };
        let document = document(
            &emulator,
            &case,
            &format!("/v2/styles/Standard/descriptor?{query}"),
        )
        .await;
        let by_id: BTreeMap<String, serde_json::Value> = document["layers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|layer| {
                layer["id"]
                    .as_str()
                    .map(|id| (id.to_string(), layer.clone()))
            })
            .collect();
        // The de-facto boundary and the alternative alignment are both part of
        // the document, whatever the request asked for.
        for id in ["admin_country", "admin_region"].into_iter().chain(disputed) {
            assert!(
                by_id.contains_key(id),
                "case {case:?}: the document must declare {id}, got {:?}",
                by_id.keys().collect::<Vec<_>>()
            );
        }
        for id in disputed {
            // The clause that narrows the layer to a view, found by the
            // attribute it reads rather than by its position, so the clause
            // list can grow without this case moving with it.
            let clause = by_id[id]["filter"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|clause| clause[1] == serde_json::json!(["get", "political_view"]))
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: layer {id} must carry a clause on the view attribute, got {}",
                        by_id[id]["filter"]
                    )
                });
            // The selected views: the one asked for, and none otherwise.
            let selected = &clause[2][1];
            assert_eq!(
                selected,
                &serde_json::json!(view.into_iter().collect::<Vec<_>>()),
                "case {case:?}: layer {id} must select the territories {view:?} claims, got {}",
                by_id[id]["filter"]
            );
            let filter = by_id[id]["filter"].to_string();
            assert!(
                filter.contains("political_view"),
                "case {case:?}: layer {id} must filter on the view attribute, got {filter}"
            );
        }
        // The de-facto boundary is not narrowed by the view at all.
        let plain = by_id["admin_country"]["filter"].to_string();
        assert!(
            !plain.contains("political_view"),
            "case {case:?}: the de-facto boundary stands whatever the view, got {plain}"
        );
    }
}

/// A layer whose tile layer no declared source carries is NOT declared: the
/// published catalogue holds families that only a knob brings into the tiles,
/// and a layer with nothing behind it would draw nothing.
#[tokio::test]
async fn a_layer_is_declared_only_when_its_tiles_carry_it() {
    let emulator = Emulator::new();
    // Row = (query, the tile layers the request puts in the tiles). Every other
    // tile layer the catalogue names must leave its layers undeclared.
    let rows: [(&str, &[&str]); 5] = [
        ("", &[]),
        ("traffic=Congestion", &["traffic_flow"]),
        ("traffic=All", &["traffic_flow", "traffic_incidents"]),
        (
            "contour-density=Medium&terrain=Hillshade&travel-modes=Truck",
            &["contour_lines", "hillshade", "logistics"],
        ),
        ("travel-modes=Transit", &["transit"]),
    ];
    // The tile layers that are in the basemap tiles whatever the request asks.
    let always: BTreeSet<&str> = BASEMAP_LAYERS
        .iter()
        .map(|spec: &StyleLayerSpec| spec.source_layer)
        .filter(|name| {
            !matches!(
                *name,
                "traffic_flow"
                    | "traffic_incidents"
                    | "contour_lines"
                    | "hillshade"
                    | "logistics"
                    | "transit"
            )
        })
        .collect();

    for (query, added) in rows {
        let case = format!("Standard?{query}");
        let document = document(
            &emulator,
            &case,
            &format!("/v2/styles/Standard/descriptor?{query}"),
        )
        .await;
        let declared: BTreeSet<&str> = document["layers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|layer| layer["id"].as_str())
            .collect();
        for spec in BASEMAP_LAYERS {
            let behind_it = always.contains(spec.source_layer) || added.contains(&spec.source_layer);
            // A gated layer is a separate rule, checked where its knob is.
            if spec.gate != LayerGate::Always {
                continue;
            }
            assert_eq!(
                declared.contains(spec.id),
                behind_it,
                "case {case:?}: layer {} reads {} \u{2014} expected declared={behind_it}, got {}",
                spec.id,
                spec.source_layer,
                declared.contains(spec.id)
            );
        }
    }
}
