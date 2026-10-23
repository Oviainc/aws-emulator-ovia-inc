// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Geo Maps map-asset routes.
//!
//! Every case drives a real HTTP request through the router the emulator mounts,
//! so what is asserted is the wire behaviour: status, headers and body bytes.
//!
//! The rules these tests pin, each over a set of inputs rather than one
//! recorded request:
//!
//! * a style descriptor is the stored style record projected through the
//!   request's knobs, and the templates it hands back are routes this service
//!   serves (the MapLibre bootstrap: descriptor -> tiles, sprites, glyphs);
//! * every asset's `ETag` is a hash of the bytes returned, so a repeat request
//!   is byte-identical and carries the same `ETag`;
//! * every asset's `Content-Type`, `Cache-Control` and `PricingBucket` come
//!   from the catalog in engine state, not from the handler;
//! * an address the catalog does not carry is a `ResourceNotFoundException`,
//!   and a parameter outside a documented rule is a `ValidationException`.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use prost::Message as _;
use service_com_amazonaws_geomaps_mapsservice::ops::types::{
    COLOR_SCHEME_VALUES, MAP_STYLE_VALUES, TILE_ADDITIONAL_FEATURE_VALUES, VARIANT_VALUES,
};
use service_com_amazonaws_geomaps_mapsservice::state::assets;
use service_com_amazonaws_geomaps_mapsservice::state::vector_tile::Tile;
use service_com_amazonaws_geomaps_mapsservice::state::{
    route_shield_width, ColorSchemeKind, MapStyleKind, TileFormat, VariantKind, CITY_DOT_BOX,
    CITY_DOT_LOWRANK_BOX, INCIDENT_ICON_BOX, MARKER_HEIGHT, MARKER_WIDTH, PLACE_ICON_BOX,
    RESTRICTION_SHIELD_BOX, SHIELD_BASE_WIDTH, SHIELD_DIGIT_WIDTH,
};
use service_com_amazonaws_geomaps_mapsservice::{
    ComAmazonawsGeomapsMapsserviceState, EmulatedService,
};
use sha2::Digest as _;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host a `geo-maps` client addresses; its first label is the
/// signing name the router routes on.
const HOST: &str = "geo-maps.us-east-1.amazonaws.com";

/// One emulator: the single shared engine plus the router that serves it.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise the maps routes, not SigV4; the signature path
        // has its own tests in the framework.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// The service's asset catalog, as a handler sees it.
    fn catalog(&self) -> engine::ServiceState<ComAmazonawsGeomapsMapsserviceState> {
        self.engine
            .state::<ComAmazonawsGeomapsMapsserviceState>()
    }

    async fn get(&self, path_and_query: &str) -> http::Response<Bytes> {
        let request = http::Request::builder()
            .method(http::Method::GET)
            .uri(format!("https://{HOST}{path_and_query}"))
            .header(http::header::HOST, HOST)
            .body(Bytes::new())
            .expect("a well-formed test request");
        self.router.dispatch(request).await
    }
}

fn header(response: &http::Response<Bytes>, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// The media type the catalog says a STORED asset (a glyph range, a sprite
/// file) is delivered with. Read from state rather than written out here, so
/// these cases pin "a stored asset's media type is the object's, not the
/// payload's" rather than one spelling of it.
fn delivered_content_type(emulator: &Emulator) -> String {
    let catalog = emulator.catalog();
    let content_type = catalog.lock().asset_delivery.content_type.clone();
    content_type
}

/// The wire error code of a failed response: restJson1 carries it in
/// `x-amzn-errortype` and in the body's `__type`.
fn error_code(response: &http::Response<Bytes>) -> String {
    let from_header = header(response, "x-amzn-errortype");
    if !from_header.is_empty() {
        return from_header.rsplit('#').next().unwrap_or_default().to_string();
    }
    serde_json::from_slice::<serde_json::Value>(response.body())
        .ok()
        .and_then(|body| {
            body.get("__type")
                .and_then(serde_json::Value::as_str)
                .map(|kind| kind.rsplit('#').next().unwrap_or_default().to_string())
        })
        .unwrap_or_default()
}

/// The body as the UTF-8 text an error message is reported in.
fn body_text(response: &http::Response<Bytes>) -> String {
    String::from_utf8_lossy(response.body()).to_string()
}

fn assert_ok(case: &str, response: &http::Response<Bytes>) {
    assert_eq!(
        response.status(),
        200,
        "case {case:?}: expected 200, got {} with body {}",
        response.status(),
        body_text(response)
    );
}

/// `ETag` as this service computes it: a quoted 128-bit prefix of the body's
/// SHA-256. Recomputed here from the bytes on the wire, so the test checks the
/// header against the body it was served with.
fn expected_etag(body: &[u8]) -> String {
    let digest = sha2::Sha256::digest(body);
    format!("\"{}\"", hex::encode(&digest[..16]))
}

/// The path and query of an absolute URL, which is what the router dispatches.
fn path_of(url: &str) -> String {
    let uri: http::Uri = url.parse().unwrap_or_else(|error| {
        panic!("case {url:?}: a style template must be a URL, got {error}");
    });
    uri.path_and_query()
        .map(ToString::to_string)
        .unwrap_or_else(|| uri.path().to_string())
}

fn expand_tile_template(template: &str, z: u32, x: u32, y: u32) -> String {
    template
        .replace("{z}", &z.to_string())
        .replace("{x}", &x.to_string())
        .replace("{y}", &y.to_string())
}

/// Decode a PNG into `(width, height, rgba)`.
fn decode_png(case: &str, bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder
        .read_info()
        .unwrap_or_else(|error| panic!("case {case:?}: body must be a PNG, got {error}"));
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buffer)
        .unwrap_or_else(|error| panic!("case {case:?}: PNG frame must decode, got {error}"));
    buffer.truncate(info.buffer_size());
    assert_eq!(
        info.color_type,
        png::ColorType::Rgba,
        "case {case:?}: expected an RGBA PNG, got {:?}",
        info.color_type
    );
    (info.width, info.height, buffer)
}

fn png_pixel(width: u32, rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let offset = (y as usize * width as usize + x as usize) * 4;
    [
        rgba[offset],
        rgba[offset + 1],
        rgba[offset + 2],
        rgba[offset + 3],
    ]
}

/// Decode a JPEG into `(width, height, rgb)`.
fn decode_jpeg(case: &str, bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut decoder = zune_jpeg::JpegDecoder::new(bytes);
    let pixels = decoder
        .decode()
        .unwrap_or_else(|error| panic!("case {case:?}: body must be a JPEG, got {error}"));
    let info = decoder
        .info()
        .unwrap_or_else(|| panic!("case {case:?}: a decoded JPEG must report its dimensions"));
    (u32::from(info.width), u32::from(info.height), pixels)
}

// ---------------------------------------------------------------------------
// GetStyleDescriptor, and the bootstrap fan-out it drives
// ---------------------------------------------------------------------------

/// Every (style, colour scheme) the catalog publishes answers with the stored
/// record's document, and every template in that document resolves to a route
/// this service serves: the MapLibre bootstrap a map viewer performs on load.
#[tokio::test]
async fn a_style_descriptor_bootstraps_tiles_sprites_and_glyphs() {
    let emulator = Emulator::new();
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            let case = format!("{}/{}", style.as_wire(), scheme.as_wire());
            let path = format!(
                "/v2/styles/{}/descriptor?color-scheme={}",
                style.as_wire(),
                scheme.as_wire()
            );
            let response = emulator.get(&path).await;
            assert_ok(&case, &response);
            assert_eq!(
                header(&response, "content-type"),
                "application/json",
                "case {case:?}: a style descriptor is a JSON document"
            );
            assert!(
                !header(&response, "cache-control").is_empty(),
                "case {case:?}: the catalog's cache directive must be reported"
            );
            assert_eq!(
                header(&response, "etag"),
                expected_etag(response.body()),
                "case {case:?}: the ETag must hash the body served"
            );

            let document: serde_json::Value = serde_json::from_slice(response.body())
                .unwrap_or_else(|error| panic!("case {case:?}: body must be JSON, got {error}"));
            assert_eq!(
                document["version"], 8,
                "case {case:?}: the style specification version"
            );
            let expected_name = format!("{} {}", style.as_wire(), scheme.as_wire());
            assert_eq!(
                document["name"].as_str(),
                Some(expected_name.as_str()),
                "case {case:?}: the document must name the stored style record"
            );

            // The sources the catalog holds for this style, followed to GetTile.
            let sources = document["sources"]
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: a style must declare sources"));
            assert!(
                !sources.is_empty(),
                "case {case:?}: a style must declare at least one source"
            );
            for (name, source) in sources {
                let template = source["tiles"][0]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: source {name} needs a tile URL"));
                assert!(
                    template.starts_with(&format!("https://{HOST}/v2/tiles/")),
                    "case {case:?}: source {name} must point at this endpoint's GetTile, got {template}"
                );
                let tile = emulator
                    .get(&path_of(&expand_tile_template(template, 3, 4, 5)))
                    .await;
                assert_ok(&format!("{case} source {name}"), &tile);
                assert!(
                    !header(&tile, "x-amz-geo-pricing-bucket").is_empty(),
                    "case {case:?}: source {name}'s tiles must report a pricing bucket"
                );
            }

            // The sprite sheet and its index, followed to GetSprites. Both are
            // stored assets, so both are delivered with the object store's own
            // media type rather than the media type of what is inside them.
            let sprite = document["sprite"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a style must declare a sprite URL"));
            let delivered = delivered_content_type(&emulator);
            for suffix in [".png", ".json"] {
                let response = emulator.get(&path_of(&format!("{sprite}{suffix}"))).await;
                assert_ok(&format!("{case} sprite {suffix}"), &response);
                assert_eq!(
                    header(&response, "content-type"),
                    delivered,
                    "case {case:?}: sprite {suffix} must be served as {delivered}"
                );
            }

            // The glyph range, followed to GetGlyphs with the font stack the
            // document's own symbol layers ask for.
            let glyphs = document["glyphs"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a style must declare a glyph URL"));
            for font in font_stacks(&document) {
                let url = glyphs
                    .replace("{fontstack}", &font.replace(' ', "%20"))
                    .replace("{range}", "0-255");
                let response = emulator.get(&path_of(&url)).await;
                assert_ok(&format!("{case} glyphs {font}"), &response);
                assert!(
                    !response.body().is_empty(),
                    "case {case:?}: glyph range {font} must carry glyphs"
                );
            }
        }
    }
}

/// The templates a style document hands back carry only a scheme the REQUEST
/// itself stated — on the URI, or in `x-forwarded-proto`. With no scheme to go
/// on they stay root-relative, because a guessed scheme is a URL the client
/// cannot fetch.
#[tokio::test]
async fn style_templates_carry_only_a_scheme_the_request_stated() {
    let emulator = Emulator::new();
    // Row = (how the request addressed the emulator, x-forwarded-proto), and
    // the prefix every template in the answer must start with.
    let cases: [(&str, Option<&str>, String); 6] = [
        // An absolute URI states its own scheme, either way.
        (
            "https://geo-maps.us-east-1.amazonaws.com/v2/styles/Standard/descriptor",
            None,
            format!("https://{HOST}/v2/"),
        ),
        (
            "http://geo-maps.us-east-1.amazonaws.com/v2/styles/Standard/descriptor",
            None,
            format!("http://{HOST}/v2/"),
        ),
        // Origin-form, the shape a server really receives: no scheme on the
        // URI, so the templates stay relative...
        ("/v2/styles/Standard/descriptor", None, "/v2/".to_string()),
        // ...unless a proxy states one.
        (
            "/v2/styles/Standard/descriptor",
            Some("http"),
            format!("http://{HOST}/v2/"),
        ),
        (
            "/v2/styles/Standard/descriptor",
            Some("HTTPS"),
            format!("https://{HOST}/v2/"),
        ),
        // A value that is no scheme is not pasted into the document.
        (
            "/v2/styles/Standard/descriptor",
            Some("javascript"),
            "/v2/".to_string(),
        ),
    ];

    for (uri, forwarded, prefix) in cases {
        let case = format!("{uri} forwarded={forwarded:?}");
        let mut builder = http::Request::builder()
            .method(http::Method::GET)
            .uri(uri)
            .header(http::header::HOST, HOST);
        if let Some(proto) = forwarded {
            builder = builder.header("x-forwarded-proto", proto);
        }
        let request = builder
            .body(Bytes::new())
            .expect("a well-formed test request");
        let response = emulator.router.dispatch(request).await;
        assert_ok(&case, &response);
        let document: serde_json::Value = serde_json::from_slice(response.body().as_ref())
            .unwrap_or_else(|error| panic!("case {case:?}: must be JSON, got {error}"));

        let mut templates: Vec<String> = Vec::new();
        for source in document["sources"].as_object().into_iter().flatten() {
            if let Some(tiles) = source.1["tiles"][0].as_str() {
                templates.push(tiles.to_string());
            }
        }
        for key in ["sprite", "glyphs"] {
            if let Some(value) = document[key].as_str() {
                templates.push(value.to_string());
            }
        }
        assert!(
            templates.len() >= 3,
            "case {case:?}: expected tile, sprite and glyph templates, got {templates:?}"
        );
        for template in &templates {
            assert!(
                template.starts_with(&prefix),
                "case {case:?}: expected a template starting {prefix}, got {template}"
            );
        }
    }
}

/// Every font stack the document's symbol layers reference.
fn font_stacks(document: &serde_json::Value) -> BTreeSet<String> {
    let mut stacks = BTreeSet::new();
    for layer in document["layers"].as_array().into_iter().flatten() {
        for font in layer["layout"]["text-font"].as_array().into_iter().flatten() {
            if let Some(name) = font.as_str() {
                stacks.insert(name.to_string());
            }
        }
    }
    stacks
}

/// Each feature knob changes the document the way the catalog says it should,
/// and every layer it adds reads a `source-layer` that really is in the tile
/// the layer's own source template serves.
#[tokio::test]
async fn style_knobs_add_layers_whose_tiles_carry_them() {
    let emulator = Emulator::new();
    // Row = one query string, and what it must put in the document.
    let cases: [(&str, &[&str]); 7] = [
        ("color-scheme=Dark", &[]),
        ("terrain=Hillshade", &["hillshade"]),
        ("terrain=Terrain3D", &[]),
        ("contour-density=High", &["contours"]),
        ("traffic=All", &["traffic-flow", "traffic-incidents"]),
        (
            "travel-modes=Transit&travel-modes=Truck",
            &["travel-mode-transit", "travel-mode-truck"],
        ),
        ("political-view=IND&buildings=Buildings3D", &["borders"]),
    ];

    for (query, expected_layers) in cases {
        let case = format!("Standard?{query}");
        let response = emulator
            .get(&format!("/v2/styles/Standard/descriptor?{query}"))
            .await;
        assert_ok(&case, &response);
        let document: serde_json::Value = serde_json::from_slice(response.body())
            .unwrap_or_else(|error| panic!("case {case:?}: body must be JSON, got {error}"));

        let ids: BTreeSet<String> = document["layers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|layer| layer["id"].as_str().map(ToString::to_string))
            .collect();
        for expected in expected_layers {
            assert!(
                ids.contains(*expected),
                "case {case:?}: expected a layer {expected}, got {ids:?}"
            );
        }

        // Every layer's source-layer must exist in the tile its source serves.
        for layer in document["layers"].as_array().into_iter().flatten() {
            let (Some(source), Some(source_layer)) = (
                layer["source"].as_str(),
                layer["source-layer"].as_str(),
            ) else {
                continue;
            };
            let template = document["sources"][source]["tiles"][0]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: source {source} needs a tile URL"));
            let tile = emulator
                .get(&path_of(&expand_tile_template(template, 2, 1, 1)))
                .await;
            assert_ok(&format!("{case} tile for {source}"), &tile);
            let decoded = Tile::decode(tile.body().as_ref()).unwrap_or_else(|error| {
                panic!("case {case:?}: source {source} must serve a vector tile, got {error}")
            });
            let names: BTreeSet<&str> = decoded
                .layers
                .iter()
                .map(|layer| layer.name.as_str())
                .collect();
            assert!(
                names.contains(source_layer),
                "case {case:?}: layer {} reads source-layer {source_layer}, which tile {source} does not carry: {names:?}",
                layer["id"]
            );
        }
    }
}

/// `PoiDensity: Off` is the one value that removes the POI symbol layer; every
/// other value keeps it and only changes its spacing.
#[tokio::test]
async fn poi_density_off_removes_the_symbol_layer() {
    let emulator = Emulator::new();
    // The model's `PoiDensity` values.
    for density in [
        "Default",
        "Dense",
        "Off",
        "Sparse",
        "VeryDense",
        "VerySparse",
    ] {
        let response = emulator
            .get(&format!(
                "/v2/styles/Standard/descriptor?poi-density={density}"
            ))
            .await;
        assert_ok(density, &response);
        let document: serde_json::Value =
            serde_json::from_slice(response.body()).expect("a JSON style document");
        let has_places = document["layers"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|layer| layer["id"] == "places");
        assert_eq!(
            has_places,
            density != "Off",
            "case {density:?}: expected the places layer to be present={}, got {has_places}",
            density != "Off"
        );
    }
}

/// A knob value outside the model's own set is refused, not silently dropped
/// from the document it was meant to change.
#[tokio::test]
async fn unknown_style_knob_values_are_refused() {
    let emulator = Emulator::new();
    for query in [
        "color-scheme=Beige",
        "terrain=Nope",
        "contour-density=Extreme",
        "traffic=Some",
        "buildings=Flat",
        "poi-density=Massive",
        "travel-modes=Hovercraft",
        "poi-categories=Volcanoes",
    ] {
        let response = emulator
            .get(&format!("/v2/styles/Standard/descriptor?{query}"))
            .await;
        assert_eq!(
            response.status(),
            400,
            "case {query:?}: expected 400, got {} with body {}",
            response.status(),
            body_text(&response)
        );
    }
    // And a style label the model's own enum does not carry.
    let response = emulator.get("/v2/styles/Nope/descriptor").await;
    assert_eq!(
        response.status(),
        400,
        "case \"Style=Nope\": expected 400, got {} with body {}",
        response.status(),
        body_text(&response)
    );
}

// ---------------------------------------------------------------------------
// GetTile
// ---------------------------------------------------------------------------

/// Every tileset in the catalog serves the format, media type, cache directive
/// and pricing bucket its record carries, and a repeated request is
/// byte-identical with the same `ETag`.
#[tokio::test]
async fn tiles_are_served_from_the_tileset_record_and_are_stable() {
    let emulator = Emulator::new();
    let tilesets: Vec<_> = {
        let catalog = emulator.catalog();
        let catalog = catalog.lock();
        catalog
            .tilesets
            .values()
            .map(|record| {
                (
                    record.id.clone(),
                    record.format,
                    record.content_type.clone(),
                    record.cache_control.clone(),
                    catalog.pricing_bucket(record.pricing_bucket.as_ref()),
                    record.min_zoom,
                    record.max_zoom,
                )
            })
            .collect()
    };
    assert!(
        tilesets.len() >= 4,
        "the catalog must publish the documented tilesets, got {}",
        tilesets.len()
    );

    for (id, format, content_type, cache_control, pricing_bucket, min_zoom, max_zoom) in tilesets {
        // Rows: the lowest zoom, a middle zoom, and the highest the record has.
        for (z, x, y) in [
            (min_zoom, 0, 0),
            ((min_zoom + max_zoom) / 2, 3, 5),
            (max_zoom, 1, 2),
        ] {
            let case = format!("{id} {z}/{x}/{y}");
            let path = format!("/v2/tiles/{id}/{z}/{x}/{y}");
            let first = emulator.get(&path).await;
            assert_ok(&case, &first);
            assert_eq!(
                header(&first, "content-type"),
                content_type,
                "case {case:?}: the media type must come from the tileset record"
            );
            assert_eq!(
                header(&first, "cache-control"),
                cache_control,
                "case {case:?}: the cache directive must come from the tileset record"
            );
            assert_eq!(
                header(&first, "x-amz-geo-pricing-bucket"),
                pricing_bucket,
                "case {case:?}: the pricing bucket must come from the catalog"
            );
            assert_eq!(
                header(&first, "etag"),
                expected_etag(first.body()),
                "case {case:?}: the ETag must hash the body served"
            );
            assert!(
                !first.body().is_empty(),
                "case {case:?}: a tile must carry bytes"
            );

            // The bytes really are the format the record claims.
            match format {
                TileFormat::VectorMvt => {
                    let tile = Tile::decode(first.body().as_ref()).unwrap_or_else(|error| {
                        panic!("case {case:?}: must decode as a vector tile, got {error}")
                    });
                    assert!(
                        !tile.layers.is_empty(),
                        "case {case:?}: a vector tile must carry layers"
                    );
                    // The tile's own attributes report the address it answers.
                    let layer = &tile.layers[0];
                    let coordinate = |key: &str| {
                        let index = layer.keys.iter().position(|k| k == key)?;
                        let feature = layer.features.first()?;
                        let position = feature
                            .tags
                            .chunks(2)
                            .find(|pair| pair[0] as usize == index)?;
                        layer.values.get(position[1] as usize)?.int_value
                    };
                    assert_eq!(
                        (coordinate("z"), coordinate("x"), coordinate("y")),
                        (Some(i64::from(z)), Some(i64::from(x)), Some(i64::from(y))),
                        "case {case:?}: the tile must carry the address it was asked for"
                    );
                }
                TileFormat::RasterJpeg => {
                    let (width, height, _) = decode_jpeg(&case, first.body());
                    assert_eq!(
                        (width, height),
                        (256, 256),
                        "case {case:?}: a raster tile is 256x256"
                    );
                }
                TileFormat::RasterDemPng => {
                    let (width, height, _) = decode_png(&case, first.body());
                    assert_eq!(
                        (width, height),
                        (256, 256),
                        "case {case:?}: a terrain tile is 256x256"
                    );
                }
            }

            // Repeat: the same address answers with the same bytes.
            let second = emulator.get(&path).await;
            assert_eq!(
                first.body(),
                second.body(),
                "case {case:?}: a repeated request must be byte-identical"
            );
            assert_eq!(
                header(&first, "etag"),
                header(&second, "etag"),
                "case {case:?}: a repeated request must carry the same ETag"
            );
        }
    }
}

/// Neighbouring addresses answer with different tiles: the bytes are a function
/// of the address, not one blob served for everything.
#[tokio::test]
async fn neighbouring_tiles_differ() {
    let emulator = Emulator::new();
    for tileset in ["vector.basemap", "raster.satellite", "raster.dem"] {
        let mut seen: BTreeSet<Vec<u8>> = BTreeSet::new();
        for (z, x, y) in [(4, 8, 5), (4, 9, 5), (4, 8, 6), (5, 8, 5)] {
            let case = format!("{tileset} {z}/{x}/{y}");
            let response = emulator
                .get(&format!("/v2/tiles/{tileset}/{z}/{x}/{y}"))
                .await;
            assert_ok(&case, &response);
            assert!(
                seen.insert(response.body().to_vec()),
                "case {case:?}: this address answered with bytes another address already served"
            );
        }
    }
}

/// A terrain tile carries a continuous elevation field: the column where two
/// horizontally adjacent tiles meet agrees to within a metre or two, and every
/// elevation is a plausible one.
#[tokio::test]
async fn terrain_tiles_are_a_continuous_elevation_field() {
    let emulator = Emulator::new();
    for (z, x, y) in [(2, 1, 1), (6, 20, 24), (10, 500, 300)] {
        let case = format!("raster.dem {z}/{x}/{y}");
        let left = emulator.get(&format!("/v2/tiles/raster.dem/{z}/{x}/{y}")).await;
        let right = emulator
            .get(&format!("/v2/tiles/raster.dem/{z}/{}/{y}", x + 1))
            .await;
        assert_ok(&case, &left);
        assert_ok(&case, &right);
        let (left_width, _, left_pixels) = decode_png(&case, left.body());
        let (right_width, _, right_pixels) = decode_png(&case, right.body());

        for row in [0_u32, 77, 255] {
            let here = assets::decode_terrarium(png_pixel(
                left_width,
                &left_pixels,
                left_width - 1,
                row,
            ));
            let across = assets::decode_terrarium(png_pixel(right_width, &right_pixels, 0, row));
            // The two samples are one pixel apart across the seam, so a
            // continuous field differs by at most its gradient times that step.
            // The field's steepest term is bounded by ~150 m per degree, and a
            // pixel at zoom z spans 360 / (256 * 2^z) degrees.
            let degrees_per_pixel = 360.0 / (256.0 * 2_f64.powi(z));
            let tolerance = 150.0 * degrees_per_pixel + 1.0;
            assert!(
                (here - across).abs() <= tolerance,
                "case {case:?} row {row}: the seam must be continuous within {tolerance} m, got {here} and {across}"
            );
            assert!(
                (-11_000.0..9_000.0).contains(&here),
                "case {case:?} row {row}: {here} is not a plausible elevation"
            );
        }
    }
}

/// `AdditionalFeatures` adds exactly the layers the model's values name.
#[tokio::test]
async fn additional_features_add_their_own_tile_layers() {
    let emulator = Emulator::new();
    // EVERY value the model declares for `TileAdditionalFeature`, taken from the
    // generated constant rather than written out here, so a value the model adds
    // later is covered by this test without editing it. The layer each one adds
    // is the member name in snake_case, which is what the handler derives.
    let cases: Vec<(&str, String)> = TILE_ADDITIONAL_FEATURE_VALUES
        .iter()
        .map(|feature| {
            let layer = assets::additional_feature_layer(feature)
                .unwrap_or_else(|| panic!("case {feature:?}: a modeled feature must map to a layer"));
            (*feature, layer)
        })
        .collect();
    assert!(
        !cases.is_empty(),
        "case \"model\": TileAdditionalFeature must declare at least one value, got {:?}",
        TILE_ADDITIONAL_FEATURE_VALUES
    );
    for (feature, layer) in &cases {
        let response = emulator
            .get(&format!(
                "/v2/tiles/vector.basemap/4/8/5?additional-features={feature}"
            ))
            .await;
        assert_ok(feature, &response);
        let tile = Tile::decode(response.body().as_ref())
            .unwrap_or_else(|error| panic!("case {feature:?}: must decode, got {error}"));
        let names: BTreeSet<&str> = tile.layers.iter().map(|l| l.name.as_str()).collect();
        assert!(
            names.contains(layer.as_str()),
            "case {feature:?}: expected the {layer} layer, got {names:?}"
        );
    }

    // All of them at once, up to the modeled `len ..4` maximum on the list.
    let query = cases
        .iter()
        .take(4)
        .map(|(feature, _)| format!("additional-features={feature}"))
        .collect::<Vec<_>>()
        .join("&");
    let response = emulator
        .get(&format!("/v2/tiles/vector.basemap/4/8/5?{query}"))
        .await;
    assert_ok("every feature at once", &response);
    let tile = Tile::decode(response.body().as_ref()).expect("a vector tile");
    let names: BTreeSet<&str> = tile.layers.iter().map(|l| l.name.as_str()).collect();
    for (_, layer) in cases.iter().take(4) {
        assert!(
            names.contains(layer.as_str()),
            "case \"every feature at once\": expected the {layer} layer, got {names:?}"
        );
    }

    // A value the model does NOT declare is refused, and the refusal is the
    // modeled `ValidationException` rather than a tile missing a layer.
    for unmodeled in ["Contourlines", "Hillshade%20", "Weather", ""] {
        let response = emulator
            .get(&format!(
                "/v2/tiles/vector.basemap/4/8/5?additional-features={unmodeled}"
            ))
            .await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {unmodeled:?}: expected 400, got {} / {}",
            response.status(),
            body_text(&response)
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {unmodeled:?}: expected ValidationException, got {}",
            body_text(&response)
        );
    }
}

/// An address the catalog does not carry is a `ResourceNotFoundException`, and
/// a coordinate outside the tileset's own grid is a `ValidationException`.
#[tokio::test]
async fn tile_addresses_outside_the_catalog_are_refused() {
    let emulator = Emulator::new();
    // Row = (path, expected status, expected error code).
    let cases: [(&str, u16, &str); 9] = [
        ("/v2/tiles/no-such-tileset/3/4/5", 404, "ResourceNotFoundException"),
        ("/v2/tiles/vector.basemap/999/0/0", 400, "ValidationException"),
        ("/v2/tiles/vector.basemap/17/0/0", 400, "ValidationException"),
        ("/v2/tiles/raster.dem/15/0/0", 400, "ValidationException"),
        ("/v2/tiles/vector.basemap/abc/0/0", 400, "ValidationException"),
        ("/v2/tiles/vector.basemap/3/-1/0", 400, "ValidationException"),
        ("/v2/tiles/vector.basemap/3/8/0", 400, "ValidationException"),
        ("/v2/tiles/vector.basemap/3/0/8", 400, "ValidationException"),
        (
            "/v2/tiles/vector.basemap/3/0/0?additional-features=NoSuchFeature",
            400,
            "ValidationException",
        ),
    ];
    for (path, status, code) in cases {
        let response = emulator.get(path).await;
        assert_eq!(
            response.status(),
            status,
            "case {path:?}: expected {status}, got {} with body {}",
            response.status(),
            body_text(&response)
        );
        assert_eq!(
            error_code(&response),
            code,
            "case {path:?}: expected {code}, got body {}",
            body_text(&response)
        );
    }

    // The zoom just inside each tileset's range is served, which is what makes
    // the refusals above about the range and not about the route.
    for (path, _, _) in [
        ("/v2/tiles/vector.basemap/16/0/0", 0, ""),
        ("/v2/tiles/raster.dem/14/0/0", 0, ""),
        ("/v2/tiles/raster.satellite/19/0/0", 0, ""),
    ] {
        let response = emulator.get(path).await;
        assert_ok(path, &response);
    }
}

// ---------------------------------------------------------------------------
// GetSprites
// ---------------------------------------------------------------------------

/// The JSON index describes the icons of the PNG it is paired with: every icon
/// the catalog holds has an entry, every entry's rectangle is inside the sheet,
/// and the pixel at the centre of each rectangle is that icon's own colour.
#[tokio::test]
async fn the_sprite_index_describes_the_sheet_it_is_paired_with() {
    let emulator = Emulator::new();
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            let icons: Vec<(String, [u8; 4])> = {
                let catalog = emulator.catalog();
                let catalog = catalog.lock();
                catalog
                    .sprites
                    .get(&(style, scheme, VariantKind::Default))
                    .expect("the catalog publishes a sheet per style and scheme")
                    .icons
                    .iter()
                    .map(|icon| (icon.name.clone(), icon.tint))
                    .collect()
            };

            // Rows: both pixel ratios the modeled file-name pattern admits.
            for (file, ratio) in [("sprites", 1_u32), ("sprites@2x", 2)] {
                let case = format!("{}/{}/{file}", style.as_wire(), scheme.as_wire());
                let base = format!(
                    "/v2/styles/{}/{}/Default/sprites/{file}",
                    style.as_wire(),
                    scheme.as_wire()
                );
                let sheet = emulator.get(&format!("{base}.png")).await;
                let index = emulator.get(&format!("{base}.json")).await;
                assert_ok(&case, &sheet);
                assert_ok(&case, &index);
                assert_eq!(
                    header(&sheet, "content-type"),
                    delivered_content_type(&emulator),
                    "case {case:?}: a stored sprite file reports the object store's media type"
                );
                assert_eq!(
                    header(&index, "content-type"),
                    delivered_content_type(&emulator),
                    "case {case:?}: a stored sprite file reports the object store's media type"
                );
                assert_eq!(
                    header(&sheet, "etag"),
                    expected_etag(sheet.body()),
                    "case {case:?}: the ETag must hash the body served"
                );

                let (width, height, pixels) = decode_png(&case, sheet.body());
                let described: serde_json::Value = serde_json::from_slice(index.body())
                    .unwrap_or_else(|error| panic!("case {case:?}: index must be JSON: {error}"));
                let described = described
                    .as_object()
                    .unwrap_or_else(|| panic!("case {case:?}: index must be a JSON object"));

                assert_eq!(
                    described.len(),
                    icons.len(),
                    "case {case:?}: the index must describe every icon in the catalog"
                );
                for (name, tint) in &icons {
                    let entry = described
                        .get(name)
                        .unwrap_or_else(|| panic!("case {case:?}: index must describe {name}"));
                    assert_eq!(
                        entry["pixelRatio"].as_u64(),
                        Some(u64::from(ratio)),
                        "case {case:?}: {name} must report the requested pixel ratio"
                    );
                    let read = |key: &str| {
                        entry[key].as_u64().unwrap_or_else(|| {
                            panic!("case {case:?}: {name} must report a numeric {key}")
                        }) as u32
                    };
                    let (x, y, w, h) = (read("x"), read("y"), read("width"), read("height"));
                    assert!(
                        x + w <= width && y + h <= height,
                        "case {case:?}: {name} at {x},{y} {w}x{h} falls outside the {width}x{height} sheet"
                    );
                    let centre = png_pixel(width, &pixels, x + w / 2, y + h / 2);
                    assert_eq!(
                        centre, *tint,
                        "case {case:?}: the pixel at the centre of {name} must be its catalog colour"
                    );
                }
            }

            // `@2x` is the same sheet at twice the size.
            let base = format!(
                "/v2/styles/{}/{}/Default/sprites/sprites",
                style.as_wire(),
                scheme.as_wire()
            );
            let one = emulator.get(&format!("{base}.json")).await;
            let two = emulator.get(&format!("{base}@2x.json")).await;
            let one: serde_json::Value = serde_json::from_slice(one.body()).expect("JSON");
            let two: serde_json::Value = serde_json::from_slice(two.body()).expect("JSON");
            for (name, entry) in one.as_object().expect("a JSON object") {
                let case = format!("{}/{}/{name}@2x", style.as_wire(), scheme.as_wire());
                assert_eq!(
                    two[name]["width"].as_u64(),
                    entry["width"].as_u64().map(|value| value * 2),
                    "case {case:?}: an @2x icon is twice as wide"
                );
                assert_eq!(
                    two[name]["x"].as_u64(),
                    entry["x"].as_u64().map(|value| value * 2),
                    "case {case:?}: an @2x icon sits at twice the offset"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// GetGlyphs
// ---------------------------------------------------------------------------

/// A glyph range is a decodable SDF `.pbf` for the stack and range requested,
/// with each glyph's bitmap the size its own metrics declare.
#[tokio::test]
async fn glyph_ranges_decode_for_every_stack_and_range() {
    let emulator = Emulator::new();
    let stacks = [
        "Amazon Ember Regular",
        "Amazon Ember Bold",
        "Amazon Ember Bold,Noto Sans Bold",
        "Not A Published Font",
    ];
    // Rows: the first range, a later one, and the last range of the BMP.
    let ranges = [(0_u32, 255_u32), (256, 511), (65280, 65535)];

    for stack in stacks {
        for (start, end) in ranges {
            let case = format!("{stack} {start}-{end}");
            let response = emulator
                .get(&format!(
                    "/v2/glyphs/{}/{start}-{end}.pbf",
                    stack.replace(' ', "%20")
                ))
                .await;
            assert_ok(&case, &response);
            assert_eq!(
                header(&response, "content-type"),
                delivered_content_type(&emulator),
                "case {case:?}: a stored glyph range reports the object store's media type"
            );
            assert_eq!(
                header(&response, "etag"),
                expected_etag(response.body()),
                "case {case:?}: the ETag must hash the body served"
            );

            let decoded =
                service_com_amazonaws_geomaps_mapsservice::state::glyph_pbf::Glyphs::decode(
                    response.body().as_ref(),
                )
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: must decode as a glyph range, got {error}")
                });
            assert_eq!(
                decoded.stacks.len(),
                1,
                "case {case:?}: a range carries exactly one font stack"
            );
            let fontstack = &decoded.stacks[0];
            assert_eq!(
                fontstack.name, stack,
                "case {case:?}: the stack must be the one requested"
            );
            assert_eq!(
                fontstack.range,
                format!("{start}-{end}"),
                "case {case:?}: the range must be the one requested"
            );
            assert!(
                !fontstack.glyphs.is_empty(),
                "case {case:?}: a range must carry glyphs"
            );
            for glyph in &fontstack.glyphs {
                assert!(
                    (start..=end).contains(&glyph.id),
                    "case {case:?}: glyph {} is outside the range", glyph.id
                );
                let bitmap = glyph
                    .bitmap
                    .as_ref()
                    .unwrap_or_else(|| panic!("case {case:?}: glyph {} needs a bitmap", glyph.id));
                // The SDF format keeps a 3px buffer on every side.
                assert_eq!(
                    bitmap.len(),
                    (glyph.width as usize + 6) * (glyph.height as usize + 6),
                    "case {case:?}: glyph {}'s bitmap must match its metrics",
                    glyph.id
                );
                assert!(
                    glyph.advance > 0,
                    "case {case:?}: glyph {} must advance the cursor",
                    glyph.id
                );
            }
        }
    }

    // Two different published stacks measure differently, so the response is
    // the stack's own metrics and not one blob for every font.
    let regular = emulator
        .get("/v2/glyphs/Amazon%20Ember%20Regular/0-255.pbf")
        .await;
    let condensed = emulator
        .get("/v2/glyphs/Amazon%20Ember%20Condensed%20RC%20Regular/0-255.pbf")
        .await;
    assert_ne!(
        regular.body(),
        condensed.body(),
        "case \"two stacks\": a condensed face must not answer with the regular face's glyphs"
    );
}

/// A glyph range must be aligned to multiples of 256, which the member
/// documents and the modeled pattern cannot express.
#[tokio::test]
async fn misaligned_glyph_ranges_are_refused() {
    let emulator = Emulator::new();
    // Rows: unaligned start, wrong width, reversed, a start past the Unicode
    // range (whose aligned end would overflow a u32), and the aligned control.
    let cases: [(&str, u16); 8] = [
        ("/v2/glyphs/Amazon%20Ember%20Regular/1-256.pbf", 400),
        ("/v2/glyphs/Amazon%20Ember%20Regular/0-127.pbf", 400),
        ("/v2/glyphs/Amazon%20Ember%20Regular/0-511.pbf", 400),
        ("/v2/glyphs/Amazon%20Ember%20Regular/512-255.pbf", 400),
        ("/v2/glyphs/Amazon%20Ember%20Regular/65536-65791.pbf", 400),
        (
            "/v2/glyphs/Amazon%20Ember%20Regular/4294967040-4294967295.pbf",
            400,
        ),
        ("/v2/glyphs/Amazon%20Ember%20Regular/0-255.pbf", 200),
        ("/v2/glyphs/Amazon%20Ember%20Regular/512-767.pbf", 200),
    ];
    for (path, status) in cases {
        let response = emulator.get(path).await;
        assert_eq!(
            response.status(),
            status,
            "case {path:?}: expected {status}, got {} with body {}",
            response.status(),
            body_text(&response)
        );
        if status == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {path:?}: a misaligned range is a validation failure"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// GetStaticMap
// ---------------------------------------------------------------------------

/// Every documented way of framing a static map renders an image of the
/// requested size, in the catalog's media type, with the required pricing
/// bucket — and a repeat of the same request is byte-identical.
#[tokio::test]
async fn static_maps_render_every_documented_framing() {
    let emulator = Emulator::new();
    // Rows: each framing form, each file name (pixel ratio), and the styling
    // knobs that change the render.
    let cases: [(&str, u32, u32, u32); 7] = [
        ("/v2/static/map?width=600&height=400&center=-123.1,49.3&zoom=10", 600, 400, 1),
        ("/v2/static/map?width=256&height=256&center=0,0&radius=5000", 256, 256, 1),
        (
            "/v2/static/map?width=800&height=600&bounding-box=-123.17,49.26,-123.08,49.31",
            800,
            600,
            1,
        ),
        (
            "/v2/static/map?width=512&height=512&bounded-positions=97.17,78.03,99.04,27.17",
            512,
            512,
            1,
        ),
        ("/v2/static/map@2x?width=320&height=240&center=13.4,52.5&zoom=12", 320, 240, 2),
        (
            "/v2/static/map?width=640&height=480&center=2.35,48.85&zoom=11&style=Satellite&color-scheme=Dark&label-size=Large&scale-unit=Miles&pois=Disabled&padding=20",
            640,
            480,
            1,
        ),
        (
            "/v2/static/map?width=400&height=400&center=-0.12,51.5&zoom=9&geojson-overlay=%7B%22type%22%3A%22Point%22%2C%22coordinates%22%3A%5B-0.12%2C51.5%5D%7D",
            400,
            400,
            1,
        ),
    ];

    let (content_type, pricing_bucket) = {
        let catalog = emulator.catalog();
        let catalog = catalog.lock();
        (
            catalog.static_maps.content_type.clone(),
            catalog.pricing_bucket(catalog.static_maps.pricing_bucket.as_ref()),
        )
    };

    for (path, width, height, ratio) in cases {
        let response = emulator.get(path).await;
        assert_ok(path, &response);
        assert_eq!(
            header(&response, "content-type"),
            content_type,
            "case {path:?}: the media type must come from the catalog"
        );
        assert_eq!(
            header(&response, "x-amz-geo-pricing-bucket"),
            pricing_bucket,
            "case {path:?}: the required pricing bucket must come from the catalog"
        );
        assert_eq!(
            header(&response, "etag"),
            expected_etag(response.body()),
            "case {path:?}: the ETag must hash the body served"
        );

        let (decoded_width, decoded_height, _) = decode_jpeg(path, response.body());
        assert_eq!(
            (decoded_width, decoded_height),
            (width * ratio, height * ratio),
            "case {path:?}: the image must be the requested size at the requested pixel ratio"
        );

        let repeat = emulator.get(path).await;
        assert_eq!(
            response.body(),
            repeat.body(),
            "case {path:?}: a repeated request must be byte-identical"
        );
    }
}

/// Framing changes the image: two different views of the world are two
/// different images, and the same view is one image.
#[tokio::test]
async fn static_map_framing_changes_the_image() {
    let emulator = Emulator::new();
    let mut seen: BTreeSet<Vec<u8>> = BTreeSet::new();
    let cases = [
        "/v2/static/map?width=256&height=256&center=-123.1,49.3&zoom=10",
        "/v2/static/map?width=256&height=256&center=-123.1,49.3&zoom=11",
        "/v2/static/map?width=256&height=256&center=13.4,52.5&zoom=10",
        "/v2/static/map?width=256&height=256&center=-123.1,49.3&radius=20000",
        "/v2/static/map?width=256&height=256&center=-123.1,49.3&zoom=10&style=Satellite",
        "/v2/static/map?width=256&height=256&center=-123.1,49.3&zoom=10&color-scheme=Dark",
    ];
    for path in cases {
        let response = emulator.get(path).await;
        assert_ok(path, &response);
        assert!(
            seen.insert(response.body().to_vec()),
            "case {path:?}: this request answered with an image another request already served"
        );
    }
}

/// The framing rules the model's own member documentation states, and the WGS 84
/// bounds a position has to satisfy.
#[tokio::test]
async fn static_map_refuses_requests_it_cannot_frame() {
    let emulator = Emulator::new();
    // Row = (path, expected status). Every refusal is a ValidationException.
    let cases: [(&str, u16); 17] = [
        // No framing parameter at all.
        ("/v2/static/map?width=600&height=400", 400),
        // `Center` requires either Zoom or Radius.
        ("/v2/static/map?width=600&height=400&center=-123.1,49.3", 400),
        // `Zoom` cannot be used with `Radius`.
        (
            "/v2/static/map?width=600&height=400&center=-123.1,49.3&zoom=10&radius=500",
            400,
        ),
        // A bounding box frames the image itself.
        (
            "/v2/static/map?width=600&height=400&bounding-box=-123.17,49.26,-123.08,49.31&zoom=10",
            400,
        ),
        (
            "/v2/static/map?width=600&height=400&bounded-positions=1,2,3,4&radius=500",
            400,
        ),
        // A bounding box is exactly two pairs.
        (
            "/v2/static/map?width=600&height=400&bounding-box=-123.17,49.26,-123.08,49.31,-122.0,48.0",
            400,
        ),
        // Exactly one framing parameter frames the image.
        (
            "/v2/static/map?width=600&height=400&bounding-box=-123.17,49.26,-123.08,49.31&center=0,0",
            400,
        ),
        // Padding is bounded by the documented min(width, height) / 4.
        (
            "/v2/static/map?width=600&height=400&center=0,0&zoom=5&padding=200",
            400,
        ),
        // An enum-valued knob outside the model's set is refused, not defaulted.
        (
            "/v2/static/map?width=600&height=400&center=0,0&zoom=5&label-size=Huge",
            400,
        ),
        // A path segment the modeled FileName pattern refuses is a validation
        // failure, not an unknown route.
        ("/v2/static/notmap?width=600&height=400&center=0,0&zoom=5", 400),
        // Longitude and latitude are bounded.
        (
            "/v2/static/map?width=600&height=400&bounding-box=-200.0,49.26,-123.08,49.31",
            400,
        ),
        ("/v2/static/map?width=600&height=400&center=10.0,95.0&zoom=5", 400),
        // `Radius` takes any value >= 1.
        ("/v2/static/map?width=600&height=400&center=0,0&radius=0", 400),
        // A GeoJSON overlay has to be GeoJSON.
        (
            "/v2/static/map?width=600&height=400&center=0,0&zoom=5&geojson-overlay=not-json",
            400,
        ),
        (
            "/v2/static/map?width=600&height=400&center=0,0&zoom=5&geojson-overlay=%7B%22type%22%3A%22Nope%22%7D",
            400,
        ),
        // An overlay this emulator cannot draw is refused, not silently
        // dropped from the image.
        (
            "/v2/static/map?width=600&height=400&center=0,0&zoom=5&compact-overlay=line%3A1,2%7C3,4",
            400,
        ),
        // The control: the same request with a framing it can use.
        ("/v2/static/map?width=600&height=400&center=0,0&zoom=5", 200),
    ];
    for (path, status) in cases {
        let response = emulator.get(path).await;
        assert_eq!(
            response.status(),
            status,
            "case {path:?}: expected {status}, got {} with body {}",
            response.status(),
            body_text(&response)
        );
        if status == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {path:?}: expected a ValidationException, got body {}",
                body_text(&response)
            );
            let body: serde_json::Value =
                serde_json::from_slice(response.body()).expect("a JSON error envelope");
            assert!(
                body.get("message").is_some(),
                "case {path:?}: a refusal must say what was wrong, got {body}"
            );
        }
    }

    // A position list past the cap the renderer plots is refused rather than
    // drawn, and one well inside it is served: the bound is a real edge, not a
    // blanket refusal of long lists.
    for (pairs, status) in [(200_usize, 200_u16), (1_300, 400)] {
        let positions = (0..pairs)
            .map(|index| format!("{},{}", index % 180, index % 80))
            .collect::<Vec<_>>()
            .join(",");
        let case = format!("bounded-positions with {pairs} pairs");
        let response = emulator
            .get(&format!(
                "/v2/static/map?width=256&height=256&bounded-positions={positions}"
            ))
            .await;
        assert_eq!(
            response.status(),
            status,
            "case {case:?}: expected {status}, got {} with body {}",
            response.status(),
            body_text(&response)
        );
    }
}

/// `Width` and `Height` are required and bounded, so a request that omits or
/// exceeds them is refused before any rendering happens — with the modeled
/// error, not merely some 4xx.
#[tokio::test]
async fn static_map_requires_a_modeled_width_and_height() {
    let emulator = Emulator::new();
    for path in [
        "/v2/static/map?center=0,0&zoom=5",
        "/v2/static/map?width=600&center=0,0&zoom=5",
        "/v2/static/map?height=400&center=0,0&zoom=5",
        "/v2/static/map?width=600&height=63&center=0,0&zoom=5",
        "/v2/static/map?width=600&height=1401&center=0,0&zoom=5",
        "/v2/static/map?width=0&height=0&center=0,0&zoom=5",
    ] {
        let response = emulator.get(path).await;
        assert_eq!(
            response.status(),
            400,
            "case {path:?}: expected 400, got {} with body {}",
            response.status(),
            body_text(&response)
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {path:?}: expected a ValidationException, got body {}",
            body_text(&response)
        );
    }
}

// ---------------------------------------------------------------------------
// The catalog is where the answers come from
// ---------------------------------------------------------------------------

/// Every catalog key enum covers exactly the values the MODEL declares, so a
/// wire value the model admits is never one the catalog cannot key on (and
/// vice versa). This is the drift check for the `from_wire`/`as_wire` pairs in
/// `state.rs`, which are the only hand-written mapping left between a modelled
/// enum and a catalog key.
#[test]
fn catalog_key_enums_cover_the_modeled_value_sets() {
    let cases: [(&str, &[&str], Vec<String>); 3] = [
        (
            "MapStyle",
            MAP_STYLE_VALUES,
            MapStyleKind::ALL
                .iter()
                .map(|style| style.as_wire().to_string())
                .collect(),
        ),
        (
            "ColorScheme",
            COLOR_SCHEME_VALUES,
            ColorSchemeKind::ALL
                .iter()
                .map(|scheme| scheme.as_wire().to_string())
                .collect(),
        ),
        (
            "Variant",
            VARIANT_VALUES,
            VariantKind::ALL
                .iter()
                .map(|variant| variant.as_wire().to_string())
                .collect(),
        ),
    ];
    for (case, modeled, catalog) in cases {
        let modeled: BTreeSet<&str> = modeled.iter().copied().collect();
        let catalog: BTreeSet<&str> = catalog.iter().map(String::as_str).collect();
        assert_eq!(
            catalog, modeled,
            "case {case:?}: the catalog keys must be exactly the modeled values, got {catalog:?}"
        );
        // And each modelled value really parses back to a catalog key.
        for value in &modeled {
            let parsed = match case {
                "MapStyle" => MapStyleKind::from_wire(value).map(|k| k.as_wire()),
                "ColorScheme" => ColorSchemeKind::from_wire(value).map(|k| k.as_wire()),
                _ => VariantKind::from_wire(value).map(|k| k.as_wire()),
            };
            assert_eq!(
                parsed,
                Some(*value),
                "case {case:?}: {value:?} must round-trip through from_wire, got {parsed:?}"
            );
        }
    }
}

/// Changing the catalog changes the responses: the assets are computed from
/// engine state, so a tileset added to state is served, one removed is a 404,
/// and a record's reported values follow the record.
#[tokio::test]
async fn responses_follow_the_catalog_in_engine_state() {
    let emulator = Emulator::new();

    // A tileset the seed does not carry, added to state.
    {
        let catalog = emulator.catalog();
        let mut catalog = catalog.lock();
        let mut added = catalog
            .tilesets
            .get("vector.basemap")
            .expect("the seeded basemap")
            .clone();
        added.id = "vector.custom".to_string();
        added.layers = vec!["custom_layer".to_string()];
        added.min_zoom = 2;
        added.max_zoom = 4;
        added.cache_control = "max-age=7".to_string();
        // No bucket of its own: it must report the catalog's default.
        added.pricing_bucket = None;
        catalog.tilesets.insert(added.id.clone(), added);
        catalog.default_pricing_bucket = "CustomBucket".to_string();
        // And one the seed does carry, removed.
        catalog.tilesets.remove("vector.traffic");
    }

    let response = emulator.get("/v2/tiles/vector.custom/3/1/1").await;
    assert_ok("vector.custom 3/1/1", &response);
    assert_eq!(
        header(&response, "cache-control"),
        "max-age=7",
        "case \"vector.custom\": the cache directive must follow the added record"
    );
    assert_eq!(
        header(&response, "x-amz-geo-pricing-bucket"),
        "CustomBucket",
        "case \"vector.custom\": a record with no bucket reports the catalog default"
    );
    let tile = Tile::decode(response.body().as_ref()).expect("a vector tile");
    let names: Vec<&str> = tile.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["custom_layer"],
        "case \"vector.custom\": the tile must carry the layers the record names"
    );

    for (path, status) in [
        ("/v2/tiles/vector.custom/1/0/0", 400),
        ("/v2/tiles/vector.custom/5/0/0", 400),
        ("/v2/tiles/vector.traffic/3/1/1", 404),
    ] {
        let response = emulator.get(path).await;
        assert_eq!(
            response.status(),
            status,
            "case {path:?}: expected {status}, got {} with body {}",
            response.status(),
            body_text(&response)
        );
    }

    // A style's stored palette decides the colours in its document.
    {
        let catalog = emulator.catalog();
        let mut catalog = catalog.lock();
        let record = catalog
            .styles
            .get_mut(&(MapStyleKind::Standard, ColorSchemeKind::Light))
            .expect("the seeded Standard Light style");
        record.palette.water = [0x01, 0x02, 0x03, 0xff];
        record.name = "Catalog Edited".to_string();
    }
    let response = emulator.get("/v2/styles/Standard/descriptor").await;
    assert_ok("edited style", &response);
    let document: serde_json::Value =
        serde_json::from_slice(response.body()).expect("a JSON style document");
    assert_eq!(
        document["name"].as_str(),
        Some("Catalog Edited"),
        "case \"edited style\": the document must report the stored name"
    );
    let water = document["layers"]
        .as_array()
        .expect("layers")
        .iter()
        .find(|layer| layer["id"] == "water")
        .expect("a water layer")["paint"]["fill-color"]
        .as_str()
        .expect("a fill colour")
        .to_string();
    assert_eq!(
        water, "#010203",
        "case \"edited style\": the paint colour must follow the stored palette"
    );
}

/// Two emulators seed the same catalog, so the same request against either one
/// answers with the same bytes: nothing about an asset depends on the process
/// it was rendered in.
#[tokio::test]
async fn two_emulators_serve_identical_assets() {
    let first = Emulator::new();
    let second = Emulator::new();
    for path in [
        "/v2/styles/Standard/descriptor?color-scheme=Dark",
        "/v2/tiles/vector.basemap/5/9/12",
        "/v2/tiles/raster.dem/5/9/12",
        "/v2/styles/Standard/Light/Default/sprites/sprites.png",
        "/v2/glyphs/Amazon%20Ember%20Regular/0-255.pbf",
        "/v2/static/map?width=256&height=256&center=0,0&zoom=4",
    ] {
        let left = first.get(path).await;
        let right = second.get(path).await;
        assert_ok(path, &left);
        assert_ok(path, &right);
        assert_eq!(
            left.body(),
            right.body(),
            "case {path:?}: two emulators must serve the same asset"
        );
        assert_eq!(
            header(&left, "etag"),
            header(&right, "etag"),
            "case {path:?}: and therefore the same ETag"
        );
    }
}

// ---------------------------------------------------------------------------
// Asset delivery: the STORED assets, and the object headers they carry
// ---------------------------------------------------------------------------

/// The server-side-encryption algorithm the catalog says the asset bucket
/// reports, read from state for the same reason as [`delivered_content_type`].
fn delivered_encryption(emulator: &Emulator) -> String {
    let catalog = emulator.catalog();
    let encryption = catalog.lock().asset_delivery.server_side_encryption.clone();
    encryption
}

/// Every address of a STORED asset: a glyph range (every font stack and range
/// the catalog answers for) and every sprite file (every style, colour scheme,
/// variant, file and pixel ratio).
fn stored_asset_paths() -> Vec<String> {
    let mut paths = vec![
        // Rows: a published stack, a second one, a fallback LIST, a stack the
        // catalog does not publish; the first range, a middle one and the last
        // range of the BMP.
        "/v2/glyphs/Amazon%20Ember%20Regular/0-255.pbf".to_string(),
        "/v2/glyphs/Amazon%20Ember%20Bold/256-511.pbf".to_string(),
        "/v2/glyphs/Amazon%20Ember%20Bold,Noto%20Sans%20Bold/3072-3327.pbf".to_string(),
        "/v2/glyphs/Not%20A%20Published%20Font/65280-65535.pbf".to_string(),
    ];
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            for file in [
                "sprites.png",
                "sprites@2x.png",
                "sprites.json",
                "sprites@2x.json",
            ] {
                paths.push(format!(
                    "/v2/styles/{}/{}/{}/sprites/{file}",
                    style.as_wire(),
                    scheme.as_wire(),
                    VariantKind::Default.as_wire()
                ));
            }
        }
    }
    paths
}

/// A stored asset is served out of the object store, so every one of them —
/// every glyph range and every sprite file — reports the object's own media type
/// and carries the delivery headers of the object version it was read from.
///
/// The version id identifies the stored content: it is the same on a repeat read
/// of one asset, the same for two assets whose bytes are the same, and different
/// as soon as the bytes are.
#[tokio::test]
async fn stored_assets_carry_the_object_store_delivery_headers() {
    let emulator = Emulator::new();
    let delivered = delivered_content_type(&emulator);
    let encryption = delivered_encryption(&emulator);
    // body digest -> the version id served for it, so the one id per content
    // rule is checked across every asset rather than per request.
    let mut ids_by_content: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

    for path in stored_asset_paths() {
        let response = emulator.get(&path).await;
        assert_ok(&path, &response);
        assert_eq!(
            header(&response, "content-type"),
            delivered,
            "case {path:?}: a stored asset reports the object store's media type, got {:?}",
            header(&response, "content-type")
        );
        assert_eq!(
            header(&response, "x-amz-server-side-encryption"),
            encryption,
            "case {path:?}: a stored asset reports how the bucket encrypts it, got {:?}",
            header(&response, "x-amz-server-side-encryption")
        );
        let version_id = header(&response, "x-amz-version-id");
        assert!(
            !version_id.is_empty(),
            "case {path:?}: a stored asset must report the object version it came from"
        );
        assert_eq!(
            header(&response, "etag"),
            expected_etag(response.body()),
            "case {path:?}: the ETag must hash the body served"
        );

        // A repeat read of one asset is the same object version.
        let repeat = emulator.get(&path).await;
        assert_eq!(
            header(&repeat, "x-amz-version-id"),
            version_id,
            "case {path:?}: a repeat read must report the same object version"
        );

        let content = expected_etag(response.body());
        match ids_by_content.get(&content) {
            Some(seen) => assert_eq!(
                seen, &version_id,
                "case {path:?}: byte-identical assets must report one version id, got {version_id:?}"
            ),
            None => {
                assert!(
                    !ids_by_content.values().any(|seen| seen == &version_id),
                    "case {path:?}: a different asset must report a different version id, got {version_id:?}"
                );
                ids_by_content.insert(content, version_id);
            }
        }
    }
}

/// The negative half of the same rule: a body this service COMPUTES from the
/// request is not an object of the asset bucket, so it keeps its own media type
/// and carries neither object header.
#[tokio::test]
async fn computed_bodies_carry_no_object_headers() {
    let emulator = Emulator::new();
    // Rows: every operation whose body is computed from the request, with the
    // media type its own catalog record declares.
    let computed: Vec<(String, String)> = {
        let catalog = emulator.catalog();
        let catalog = catalog.lock();
        let mut rows = vec![
            (
                "/v2/styles/Standard/descriptor".to_string(),
                catalog
                    .styles
                    .get(&(MapStyleKind::Standard, ColorSchemeKind::Light))
                    .expect("the catalog publishes the Standard Light style")
                    .content_type
                    .clone(),
            ),
            (
                "/v2/static/map?width=256&height=256&center=0,0&zoom=4".to_string(),
                catalog.static_maps.content_type.clone(),
            ),
        ];
        for (id, record) in &catalog.tilesets {
            rows.push((
                format!("/v2/tiles/{id}/4/5/6"),
                record.content_type.clone(),
            ));
        }
        rows
    };

    for (path, expected_type) in computed {
        let response = emulator.get(&path).await;
        assert_ok(&path, &response);
        assert_eq!(
            header(&response, "content-type"),
            expected_type,
            "case {path:?}: a computed body keeps its own media type, got {:?}",
            header(&response, "content-type")
        );
        for object_header in ["x-amz-version-id", "x-amz-server-side-encryption"] {
            assert!(
                response.headers().get(object_header).is_none(),
                "case {path:?}: a computed body carries no {object_header}, got {:?}",
                header(&response, object_header)
            );
        }
    }
}

/// A glyph range is derived from the request: no two (stack, range) addresses
/// answer with the same bytes, so none of them is a constant placeholder.
#[tokio::test]
async fn glyph_ranges_differ_per_stack_and_range() {
    let emulator = Emulator::new();
    // Rows: the same range across stacks with different metrics, and the same
    // stack across ranges.
    let stacks = [
        "Amazon Ember Regular",
        "Amazon Ember Bold",
        "Amazon Ember Condensed RC Regular",
        "Noto Sans Regular",
    ];
    let ranges = ["0-255.pbf", "256-511.pbf", "3072-3327.pbf", "65280-65535.pbf"];
    let mut seen: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();

    for stack in stacks {
        for range in ranges {
            let case = format!("{stack} {range}");
            let path = format!("/v2/glyphs/{}/{range}", stack.replace(' ', "%20"));
            let response = emulator.get(&path).await;
            assert_ok(&case, &response);
            let digest = expected_etag(response.body());
            if let Some(other) = seen.get(&digest) {
                panic!("case {case:?}: must not serve the same glyph bytes as {other:?}");
            }
            seen.insert(digest, case);
        }
    }
}

/// The sprite index is the published iconography of the sheet it is paired
/// with: every icon family the catalog carries is in it, under the name a style
/// layer references, with the five members a client reads a rectangle from.
#[tokio::test]
async fn the_sprite_index_lists_the_published_iconography() {
    let emulator = Emulator::new();
    // Rows: one icon of every family the catalog holds, with the box its family
    // occupies at pixelRatio 1 (the constants in `state.rs`).
    let families: [(&str, u32, u32); 7] = [
        ("accommodation", PLACE_ICON_BOX, PLACE_ICON_BOX),
        ("truck-weigh-station", PLACE_ICON_BOX, PLACE_ICON_BOX),
        ("road_closure", INCIDENT_ICON_BOX, INCIDENT_ICON_BOX),
        ("citydot", CITY_DOT_BOX, CITY_DOT_BOX),
        ("citydot_lowrank", CITY_DOT_LOWRANK_BOX, CITY_DOT_LOWRANK_BOX),
        ("marker_transit", MARKER_WIDTH, MARKER_HEIGHT),
        (
            "shield-hgv-weight_restriction",
            RESTRICTION_SHIELD_BOX,
            RESTRICTION_SHIELD_BOX,
        ),
    ];

    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            for (file, ratio) in [("sprites.json", 1_u32), ("sprites@2x.json", 2)] {
                let case = format!("{}/{}/{file}", style.as_wire(), scheme.as_wire());
                let response = emulator
                    .get(&format!(
                        "/v2/styles/{}/{}/Default/sprites/{file}",
                        style.as_wire(),
                        scheme.as_wire()
                    ))
                    .await;
                assert_ok(&case, &response);
                let index: serde_json::Value = serde_json::from_slice(response.body())
                    .unwrap_or_else(|error| panic!("case {case:?}: index must be JSON: {error}"));
                let index = index
                    .as_object()
                    .unwrap_or_else(|| panic!("case {case:?}: index must be a JSON object"));

                for (name, width, height) in families {
                    let entry = index.get(name).unwrap_or_else(|| {
                        panic!("case {case:?}: the index must describe {name}")
                    });
                    let entry = entry.as_object().unwrap_or_else(|| {
                        panic!("case {case:?}: {name} must be an object, got {entry}")
                    });
                    let members: BTreeSet<&str> =
                        entry.keys().map(String::as_str).collect();
                    assert_eq!(
                        members,
                        ["height", "pixelRatio", "width", "x", "y"]
                            .into_iter()
                            .collect::<BTreeSet<&str>>(),
                        "case {case:?}: {name} must carry exactly the index members, got {members:?}"
                    );
                    assert_eq!(
                        entry["width"].as_u64(),
                        Some(u64::from(width * ratio)),
                        "case {case:?}: {name} is {width} style pixels wide at ratio {ratio}"
                    );
                    assert_eq!(
                        entry["height"].as_u64(),
                        Some(u64::from(height * ratio)),
                        "case {case:?}: {name} is {height} style pixels tall at ratio {ratio}"
                    );
                    assert_eq!(
                        entry["pixelRatio"].as_u64(),
                        Some(u64::from(ratio)),
                        "case {case:?}: {name} must report the requested pixel ratio"
                    );
                }
            }
        }
    }
}

/// A route shield is as wide as the longest route number its own name says it
/// has to hold: a width word (`-long`, `-xlong`, `-wide`) or a trailing
/// digit-capacity suffix, over the catalog's two-digit base.
#[test]
fn route_shield_boxes_follow_the_digit_capacity_in_the_name() {
    let two = SHIELD_BASE_WIDTH;
    let three = SHIELD_BASE_WIDTH + SHIELD_DIGIT_WIDTH;
    let four = SHIELD_BASE_WIDTH + 2 * SHIELD_DIGIT_WIDTH;
    // Rows: no suffix, each width word, each numeric capacity suffix — and
    // names the catalog does not carry, because this is the rule and not a
    // lookup of the published list.
    let cases: [(&str, u32); 12] = [
        ("shield-AE", two),
        ("shield-Emirates routes", two),
        ("shield-US-Hwy-1-2", two),
        ("shield-US-Hwy-3", three),
        ("shield-autostrade-2-3", three),
        ("shield-MX-Federal-3-4", four),
        ("shield-rectangle-white-long", three),
        ("shield-rectangle-white-xlong", four),
        ("shield-IN-National-wide", three),
        // Not in the published catalog: the rule still answers.
        ("shield-ZZ", two),
        ("shield-ZZ-long", three),
        ("shield-ZZ-9", four),
    ];
    for (name, expected) in cases {
        let actual = route_shield_width(name);
        assert_eq!(
            actual, expected,
            "case {name:?}: expected a {expected}px box, got {actual:?}"
        );
    }
}

/// The published catalog really is laid out by that rule: the index reports each
/// shield at the width its name states, and every rectangle it reports lies
/// inside the sheet it is paired with.
#[tokio::test]
async fn the_sheet_holds_every_icon_the_index_points_at() {
    let emulator = Emulator::new();
    let names: Vec<String> = {
        let catalog = emulator.catalog();
        let catalog = catalog.lock();
        catalog
            .sprites
            .get(&(
                MapStyleKind::Standard,
                ColorSchemeKind::Light,
                VariantKind::Default,
            ))
            .expect("the catalog publishes the Standard Light sheet")
            .icons
            .iter()
            .map(|icon| icon.name.clone())
            .collect()
    };
    assert!(
        names.len() > 100,
        "case \"catalog\": a published sheet carries the whole iconography, got {} icons",
        names.len()
    );

    let base = "/v2/styles/Standard/Light/Default/sprites/sprites";
    let sheet = emulator.get(&format!("{base}.png")).await;
    let index = emulator.get(&format!("{base}.json")).await;
    assert_ok("sheet", &sheet);
    assert_ok("index", &index);
    let (width, height, _) = decode_png("sheet", sheet.body());
    let described: serde_json::Value =
        serde_json::from_slice(index.body()).expect("the index must be JSON");
    let described = described.as_object().expect("the index must be an object");

    for name in &names {
        let entry = described
            .get(name)
            .unwrap_or_else(|| panic!("case {name:?}: the index must describe it"));
        let read = |key: &str| {
            entry[key]
                .as_u64()
                .unwrap_or_else(|| panic!("case {name:?}: must report a numeric {key}")) as u32
        };
        let (x, y, w, h) = (read("x"), read("y"), read("width"), read("height"));
        assert!(
            x + w <= width && y + h <= height,
            "case {name:?}: {x},{y} {w}x{h} falls outside the {width}x{height} sheet"
        );
        if let Some(rest) = name.strip_prefix("shield-") {
            if !rest.starts_with("hgv-") {
                assert_eq!(
                    w,
                    route_shield_width(name),
                    "case {name:?}: a route shield is as wide as its name states, got {w:?}"
                );
            }
        }
    }
}
