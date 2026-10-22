// SPDX-License-Identifier: CC0-1.0

//! Deriving each asset's bytes from the catalog record it belongs to.
//!
//! Every function here is a pure function of (catalog record, request
//! addressing): same inputs, same bytes. That is what makes `ETag` stable
//! across repeat requests without storing a rendered asset anywhere, and it is
//! why no handler in this service writes to state.
//!
//! The geography is shared between assets rather than invented per operation:
//! [`elevation_at`] is the one terrain field, so a `raster.dem` tile, a
//! `raster.satellite` tile and a static map all agree about where the land is.

use std::fmt::Write as _;

use prost::Message as _;
use sha2::{Digest as _, Sha256};

use super::canvas::Canvas;
use super::vector_tile::{GeomType, GeometryWriter, LayerBuilder, TagValue, Tile};
// The generated list of `TileAdditionalFeature`'s members: the ONE place this
// crate decides which strings that enum admits.
use crate::ops::types::{
    POI_CATEGORY_VALUES, STATIC_MAP_STYLE_VALUES, TILE_ADDITIONAL_FEATURE_VALUES,
};
use super::{
    ColorSchemeKind, ComAmazonawsGeomapsMapsserviceState, FontRecord, IconShape, MapStyleKind,
    Palette, Rgba, SpriteSheetRecord, StyleRecord, TileFormat, TilesetRecord, VariantKind,
};

/// Earth's equatorial circumference in metres, the constant that turns a
/// normalized Web Mercator span into ground distance.
pub const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.686;

/// The tile size, in pixels, of every raster tile this service serves and the
/// unit Web Mercator zoom levels are defined in.
pub const TILE_SIZE: u32 = 256;

/// The `GetGlyphs` URL template a style document hands a client. `{fontstack}`
/// and `{range}` are the placeholders MapLibre expands.
pub const GLYPH_URL_TEMPLATE: &str = "/v2/glyphs/{fontstack}/{range}.pbf";

/// How far off the image a projected pixel coordinate may land.
///
/// A geographic position outside the framed envelope projects outside the
/// image, and the renderer clips it away; bounding the coordinate first keeps
/// every later addition on it inside `i64` by a wide margin.
pub const MAX_PIXEL_COORDINATE: f64 = 1e9;

/// The `GetTile` URL template for `tileset`, with the `{z}/{x}/{y}`
/// placeholders a client expands. The one place this service writes the tile
/// route, so a style document can never point at a route `GetTile` does not
/// serve.
pub fn tile_url_template(tileset: &str) -> String {
    format!("/v2/tiles/{tileset}/{{z}}/{{x}}/{{y}}")
}

/// The `GetSprites` URL template for one sheet, without the `.png`/`.json`
/// suffix a client appends (the MapLibre sprite convention).
pub fn sprite_url_template(
    style: MapStyleKind,
    scheme: ColorSchemeKind,
    variant: VariantKind,
) -> String {
    format!(
        "/v2/styles/{}/{}/{}/sprites/sprites",
        style.as_wire(),
        scheme.as_wire(),
        variant.as_wire()
    )
}

/// The `ETag` for a response body: a quoted 128-bit prefix of its SHA-256
/// digest.
///
/// A hash of the bytes actually returned, so two requests that answer with the
/// same content answer with the same `ETag` and a change to any byte changes
/// it. Nothing about it is random or stored.
pub fn etag(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("\"{}\"", hex::encode(&digest[..16]))
}

/// A member of the request a handler-level check refused, and why.
///
/// `reason` is one of the model's `ValidationExceptionReason` values, which the
/// handler puts straight on the modeled `ValidationException`.
#[derive(Debug, Clone)]
pub struct FieldFault {
    pub name: String,
    pub reason: &'static str,
    pub message: String,
}

impl FieldFault {
    pub fn new(name: &str, reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            reason,
            message: message.into(),
        }
    }
}

/// A `(longitude, latitude)` pair in WGS 84, the order every Geo Maps position
/// parameter is written in.
pub type Position = (f64, f64);

// ---------------------------------------------------------------------------
// Web Mercator
// ---------------------------------------------------------------------------

/// Longitude to a normalized Web Mercator x in `0..=1`.
pub fn merc_x(lon: f64) -> f64 {
    (lon + 180.0) / 360.0
}

/// Latitude to a normalized Web Mercator y in `0..=1` (0 is north).
pub fn merc_y(lat: f64) -> f64 {
    let clamped = lat.clamp(-85.051_128_78, 85.051_128_78);
    let radians = clamped.to_radians();
    0.5 - (radians.tan() + 1.0 / radians.cos()).ln() / (2.0 * std::f64::consts::PI)
}

/// Normalized Web Mercator x back to longitude.
pub fn inv_merc_x(x: f64) -> f64 {
    x * 360.0 - 180.0
}

/// Normalized Web Mercator y back to latitude.
pub fn inv_merc_y(y: f64) -> f64 {
    let n = std::f64::consts::PI * (1.0 - 2.0 * y);
    n.sinh().atan().to_degrees()
}

/// The geographic area an image covers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Envelope {
    pub min_lon: f64,
    pub min_lat: f64,
    pub max_lon: f64,
    pub max_lat: f64,
}

impl Envelope {
    /// The envelope enclosing every position, or `None` for an empty slice.
    pub fn of(positions: &[Position]) -> Option<Self> {
        let (first_lon, first_lat) = *positions.first()?;
        let mut envelope = Self {
            min_lon: first_lon,
            min_lat: first_lat,
            max_lon: first_lon,
            max_lat: first_lat,
        };
        for &(lon, lat) in positions {
            envelope.min_lon = envelope.min_lon.min(lon);
            envelope.max_lon = envelope.max_lon.max(lon);
            envelope.min_lat = envelope.min_lat.min(lat);
            envelope.max_lat = envelope.max_lat.max(lat);
        }
        Some(envelope)
    }

    /// The envelope an image of `width` x `height` pixels covers when centred
    /// on `center` at `zoom`, by the Web Mercator definition of a zoom level:
    /// the whole world is `TILE_SIZE * 2^zoom` pixels across.
    pub fn from_center_zoom(center: Position, zoom: f64, width: u32, height: u32) -> Self {
        let world_pixels = f64::from(TILE_SIZE) * 2_f64.powf(zoom);
        let half_x = f64::from(width) / 2.0 / world_pixels;
        let half_y = f64::from(height) / 2.0 / world_pixels;
        Self::from_mercator_box(center, half_x, half_y)
    }

    /// The envelope an image covers when it must show `radius` metres around
    /// `center`: the circle is inscribed, so the shorter image dimension spans
    /// exactly the diameter.
    pub fn from_center_radius(center: Position, radius: f64, width: u32, height: u32) -> Self {
        // One normalized Mercator unit is the equatorial circumference, shrunk
        // by cos(latitude) at the latitude being measured.
        let metres_per_unit = EARTH_CIRCUMFERENCE_M * center.1.to_radians().cos().abs().max(1e-6);
        let units = (radius / metres_per_unit).max(1e-9);
        let shorter = f64::from(width.min(height)).max(1.0);
        let half_x = units * f64::from(width) / shorter;
        let half_y = units * f64::from(height) / shorter;
        Self::from_mercator_box(center, half_x, half_y)
    }

    fn from_mercator_box(center: Position, half_x: f64, half_y: f64) -> Self {
        let cx = merc_x(center.0);
        let cy = merc_y(center.1);
        Self {
            min_lon: inv_merc_x((cx - half_x).clamp(0.0, 1.0)),
            max_lon: inv_merc_x((cx + half_x).clamp(0.0, 1.0)),
            max_lat: inv_merc_y((cy - half_y).clamp(0.0, 1.0)),
            min_lat: inv_merc_y((cy + half_y).clamp(0.0, 1.0)),
        }
    }

    /// The same envelope, widened just enough that it has area: a single
    /// position, or two positions on one meridian, frames a tiny box rather
    /// than a degenerate one.
    pub fn padded(self) -> Self {
        let mut widened = self;
        if (widened.max_lon - widened.min_lon).abs() < 1e-6 {
            widened.min_lon -= 5e-4;
            widened.max_lon += 5e-4;
        }
        if (widened.max_lat - widened.min_lat).abs() < 1e-6 {
            widened.min_lat -= 5e-4;
            widened.max_lat += 5e-4;
        }
        widened
    }

    pub fn center(self) -> Position {
        (
            (self.min_lon + self.max_lon) / 2.0,
            inv_merc_y((merc_y(self.min_lat) + merc_y(self.max_lat)) / 2.0),
        )
    }

    /// Project a position into pixel coordinates of a `width` x `height` image
    /// covering this envelope.
    ///
    /// A position far outside the envelope projects far off the image; the
    /// result is clamped to [`MAX_PIXEL_COORDINATE`] so every coordinate the
    /// renderer works with stays an ordinary number that arithmetic on it
    /// cannot overflow. The clamp is orders of magnitude beyond any canvas, so
    /// it never moves a visible point.
    pub fn project(self, position: Position, width: u32, height: u32) -> (i64, i64) {
        let x0 = merc_x(self.min_lon);
        let x1 = merc_x(self.max_lon);
        let y_top = merc_y(self.max_lat);
        let y_bottom = merc_y(self.min_lat);
        let span_x = (x1 - x0).abs().max(1e-12);
        let span_y = (y_bottom - y_top).abs().max(1e-12);
        let px = (merc_x(position.0) - x0) / span_x * f64::from(width);
        let py = (merc_y(position.1) - y_top) / span_y * f64::from(height);
        let pixel = |value: f64| {
            if value.is_nan() {
                0
            } else {
                value
                    .round()
                    .clamp(-MAX_PIXEL_COORDINATE, MAX_PIXEL_COORDINATE) as i64
            }
        };
        (pixel(px), pixel(py))
    }

    /// The position at a fractional point of the image, `(0,0)` being its
    /// top-left corner.
    pub fn unproject_fraction(self, fx: f64, fy: f64) -> Position {
        let x0 = merc_x(self.min_lon);
        let x1 = merc_x(self.max_lon);
        let y_top = merc_y(self.max_lat);
        let y_bottom = merc_y(self.min_lat);
        (
            inv_merc_x(x0 + (x1 - x0) * fx),
            inv_merc_y(y_top + (y_bottom - y_top) * fy),
        )
    }

    /// Ground metres per pixel at the envelope's centre latitude.
    pub fn metres_per_pixel(self, width: u32) -> f64 {
        let span = (merc_x(self.max_lon) - merc_x(self.min_lon)).abs();
        let latitude = self.center().1.to_radians().cos().abs().max(1e-6);
        span * EARTH_CIRCUMFERENCE_M * latitude / f64::from(width.max(1))
    }
}

// ---------------------------------------------------------------------------
// Terrain
// ---------------------------------------------------------------------------

/// The emulator's terrain field: elevation in metres at a WGS 84 position.
///
/// A smooth, continuous closed form, so it is the same field no matter which
/// asset samples it: two adjacent `raster.dem` tiles agree along their shared
/// edge, and a static map's coastline is the same coastline a DEM tile reports.
pub fn elevation_at(lon: f64, lat: f64) -> f64 {
    let lon_r = lon.to_radians();
    let lat_r = lat.to_radians();
    1200.0 * (3.0 * lon_r).sin() * (2.0 * lat_r).cos()
        + 450.0 * (7.0 * lat_r).sin()
        + 300.0 * (5.0 * lon_r + 2.0 * lat_r).cos()
}

/// Elevation in the Terrarium encoding: `(r * 256 + g + b / 256) - 32768`
/// metres, the encoding a `raster-dem` source with `"encoding": "terrarium"`
/// decodes.
fn terrarium(elevation: f64) -> Rgba {
    let value = (elevation + 32768.0).clamp(0.0, 65535.999);
    let whole = value.floor();
    let red = (whole / 256.0).floor();
    let green = whole - red * 256.0;
    let blue = ((value - whole) * 256.0).floor();
    [red as u8, green as u8, blue as u8, 0xff]
}

/// The colour a surface at `elevation` is painted, shading land upward from
/// `palette.land` towards `palette.building` and painting anything below sea
/// level `palette.water`.
fn terrain_color(palette: &Palette, elevation: f64) -> Rgba {
    if elevation < 0.0 {
        return palette.water;
    }
    let factor = (elevation / 2000.0).clamp(0.0, 1.0);
    blend(palette.land, palette.building, factor)
}

fn blend(from: Rgba, to: Rgba, factor: f64) -> Rgba {
    let mix = |a: u8, b: u8| {
        (f64::from(a) + (f64::from(b) - f64::from(a)) * factor)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    [
        mix(from[0], to[0]),
        mix(from[1], to[1]),
        mix(from[2], to[2]),
        0xff,
    ]
}

/// `#rrggbb`, or `rgba(r, g, b, a)` when the colour is not opaque — the two
/// colour literals the MapLibre style specification accepts.
fn css_color(color: Rgba) -> String {
    if color[3] == 0xff {
        let mut text = String::from("#");
        for channel in &color[..3] {
            let _ = write!(text, "{channel:02x}");
        }
        text
    } else {
        format!(
            "rgba({}, {}, {}, {:.3})",
            color[0],
            color[1],
            color[2],
            f64::from(color[3]) / 255.0
        )
    }
}

// ---------------------------------------------------------------------------
// GetStyleDescriptor
// ---------------------------------------------------------------------------

/// The request knobs `GetStyleDescriptor` projects onto a stored style record.
#[derive(Debug, Clone, Default)]
pub struct StyleOptions {
    pub political_view: Option<String>,
    pub terrain: Option<String>,
    pub contour_density: Option<String>,
    pub traffic: Option<String>,
    pub travel_modes: Vec<String>,
    pub buildings: Option<String>,
    pub poi_density: Option<String>,
    pub poi_categories: Vec<String>,
}

/// The vector-tile layer each `TileAdditionalFeature` adds to a tile, so a
/// style layer and the tile it reads always name the same layer.
///
/// Whether a string IS a feature is decided by the model and nothing else:
/// `TILE_ADDITIONAL_FEATURE_VALUES` is the generated list of
/// `TileAdditionalFeature`'s members, so a value the model adds later is
/// accepted here — and refused with the same list `GetTile`'s refusal message
/// quotes — without this function being edited. The layer name is then DERIVED
/// from the member name (PascalCase to snake_case), which is the convention the
/// tileset records' own layer names follow, so there is no second hand-written
/// table to drift from the first.
pub fn additional_feature_layer(feature: &str) -> Option<String> {
    if !TILE_ADDITIONAL_FEATURE_VALUES.contains(&feature) {
        return None;
    }
    Some(snake_case(feature))
}

/// `ContourLines` -> `contour_lines`: a word boundary is an uppercase letter
/// that follows a lowercase one or a digit.
fn snake_case(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    let mut previous: Option<char> = None;
    for ch in value.chars() {
        if ch.is_ascii_uppercase() && previous.is_some_and(|p| p.is_lowercase() || p.is_numeric()) {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
        previous = Some(ch);
    }
    out
}

/// The `AdditionalFeatures` a style's basemap source must request for the
/// layers these options switch on to exist in the tiles it fetches.
fn requested_features(options: &StyleOptions) -> Vec<&'static str> {
    let mut features = Vec::new();
    if options.contour_density.is_some() {
        features.push("ContourLines");
    }
    if options.terrain.as_deref() == Some("Hillshade") {
        features.push("Hillshade");
    }
    if options.travel_modes.iter().any(|mode| mode == "Transit") {
        features.push("Transit");
    }
    if options.travel_modes.iter().any(|mode| mode == "Truck") {
        features.push("Logistics");
    }
    features
}

/// The style descriptor document for a stored style record, projected through
/// the request's feature knobs.
///
/// `base` is the absolute endpoint prefix to put in front of each template
/// (`https://<host>`); with `None` the templates stay root-relative paths.
pub fn style_descriptor(
    state: &ComAmazonawsGeomapsMapsserviceState,
    record: &StyleRecord,
    options: &StyleOptions,
    base: Option<&str>,
) -> Vec<u8> {
    let absolute = |template: &str| match base {
        Some(prefix) => format!("{prefix}{template}"),
        None => template.to_string(),
    };
    let features = requested_features(options);

    let mut sources = serde_json::Map::new();
    let mut layers: Vec<serde_json::Value> = Vec::new();
    layers.push(serde_json::json!({
        "id": "background",
        "type": "background",
        "paint": { "background-color": css_color(record.palette.land) },
    }));

    let mut basemap_source: Option<String> = None;
    for source in &record.sources {
        let Some(tileset) = state.tilesets.get(&source.tileset) else {
            continue;
        };
        let mut url = absolute(&source.tile_url_template);
        if tileset.format == TileFormat::VectorMvt && !features.is_empty() {
            // A Smithy `@httpQuery` list is REPEATED parameters, not one
            // comma-joined value, so the template a client follows must repeat
            // the parameter — otherwise `GetTile` reads one unknown feature
            // name and refuses the request.
            let query: Vec<String> = features
                .iter()
                .map(|feature| format!("additional-features={feature}"))
                .collect();
            url.push('?');
            url.push_str(&query.join("&"));
        }
        sources.insert(source.name.clone(), source_entry(tileset, &url));
        match tileset.format {
            TileFormat::VectorMvt => {
                basemap_source = Some(source.name.clone());
            }
            TileFormat::RasterJpeg | TileFormat::RasterDemPng => layers.push(serde_json::json!({
                "id": format!("{}-imagery", source.name),
                "type": "raster",
                "source": source.name,
            })),
        }
    }

    if let Some(basemap) = basemap_source.clone() {
        layers.extend(basemap_layers(record, options, &basemap, &features));
    }

    // Traffic is its own tileset, so asking for it adds a source as well as a
    // layer.
    if let (Some(traffic), Some(tileset)) = (
        options.traffic.as_deref(),
        state.tilesets.get("vector.traffic"),
    ) {
        let url = absolute(&tile_url_template(&tileset.id));
        sources.insert("traffic".to_string(), source_entry(tileset, &url));
        layers.push(serde_json::json!({
            "id": "traffic-flow",
            "type": "line",
            "source": "traffic",
            "source-layer": "traffic_flow",
            "paint": { "line-color": css_color(record.palette.road), "line-width": 2 },
        }));
        if traffic == "All" {
            layers.push(serde_json::json!({
                "id": "traffic-incidents",
                "type": "line",
                "source": "traffic",
                "source-layer": "traffic_incidents",
                "paint": { "line-color": css_color(record.palette.marker), "line-width": 3 },
            }));
        }
    }

    // 3D terrain is elevation data, which is the DEM tileset rather than a
    // layer of the basemap.
    let mut terrain_property = None;
    if options.terrain.as_deref() == Some("Terrain3D") {
        if let Some(tileset) = state.tilesets.get("raster.dem") {
            let url = absolute(&tile_url_template(&tileset.id));
            sources.insert("terrain".to_string(), source_entry(tileset, &url));
            terrain_property = Some(serde_json::json!({
                "source": "terrain",
                "exaggeration": 1.0,
            }));
        }
    }

    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "aws:style".to_string(),
        record.style.as_wire().to_string().into(),
    );
    metadata.insert(
        "aws:color-scheme".to_string(),
        record.color_scheme.as_wire().to_string().into(),
    );
    for (key, value) in [
        ("aws:political-view", options.political_view.as_deref()),
        ("aws:terrain", options.terrain.as_deref()),
        ("aws:contour-density", options.contour_density.as_deref()),
        ("aws:traffic", options.traffic.as_deref()),
        ("aws:buildings", options.buildings.as_deref()),
        ("aws:poi-density", options.poi_density.as_deref()),
    ] {
        if let Some(value) = value {
            metadata.insert(key.to_string(), value.to_string().into());
        }
    }
    if !options.travel_modes.is_empty() {
        metadata.insert(
            "aws:travel-modes".to_string(),
            options.travel_modes.clone().into(),
        );
    }
    if !options.poi_categories.is_empty() {
        metadata.insert(
            "aws:poi-categories".to_string(),
            options.poi_categories.clone().into(),
        );
    }

    let mut document = serde_json::Map::new();
    document.insert("version".to_string(), 8.into());
    document.insert("name".to_string(), record.name.clone().into());
    document.insert("metadata".to_string(), metadata.into());
    document.insert("sources".to_string(), sources.into());
    document.insert(
        "sprite".to_string(),
        absolute(&record.sprite_url_template).into(),
    );
    document.insert(
        "glyphs".to_string(),
        absolute(&record.glyph_url_template).into(),
    );
    if let Some(terrain) = terrain_property {
        document.insert("terrain".to_string(), terrain);
    }
    document.insert("layers".to_string(), layers.into());

    to_json_bytes(&serde_json::Value::Object(document))
}

/// A style-document source entry for one tileset: its type, its tile template
/// and the zoom range the tileset actually has tiles for.
fn source_entry(tileset: &TilesetRecord, url: &str) -> serde_json::Value {
    let mut entry = serde_json::Map::new();
    entry.insert(
        "type".to_string(),
        match tileset.format {
            TileFormat::VectorMvt => "vector",
            TileFormat::RasterJpeg => "raster",
            TileFormat::RasterDemPng => "raster-dem",
        }
        .into(),
    );
    entry.insert(
        "tiles".to_string(),
        serde_json::Value::Array(vec![url.to_string().into()]),
    );
    entry.insert("minzoom".to_string(), tileset.min_zoom.into());
    entry.insert("maxzoom".to_string(), tileset.max_zoom.into());
    if tileset.format != TileFormat::VectorMvt {
        entry.insert("tileSize".to_string(), TILE_SIZE.into());
    }
    if tileset.format == TileFormat::RasterDemPng {
        entry.insert("encoding".to_string(), "terrarium".into());
    }
    serde_json::Value::Object(entry)
}

/// The layers a vector basemap source contributes, after the request's knobs.
fn basemap_layers(
    record: &StyleRecord,
    options: &StyleOptions,
    source: &str,
    features: &[&'static str],
) -> Vec<serde_json::Value> {
    let palette = &record.palette;
    let mut layers = vec![
        serde_json::json!({
            "id": "land",
            "type": "fill",
            "source": source,
            "source-layer": "land",
            "paint": { "fill-color": css_color(palette.land) },
        }),
        serde_json::json!({
            "id": "water",
            "type": "fill",
            "source": source,
            "source-layer": "water",
            "paint": { "fill-color": css_color(palette.water) },
        }),
        serde_json::json!({
            "id": "roads",
            "type": "line",
            "source": source,
            "source-layer": "roads",
            "paint": { "line-color": css_color(palette.road), "line-width": 1.5 },
        }),
    ];

    // `Buildings3D` is the one value the model declares, and it is what turns
    // the footprint fill into an extrusion.
    layers.push(if options.buildings.as_deref() == Some("Buildings3D") {
        serde_json::json!({
            "id": "buildings",
            "type": "fill-extrusion",
            "source": source,
            "source-layer": "buildings",
            "paint": {
                "fill-extrusion-color": css_color(palette.building),
                "fill-extrusion-height": ["get", "height"],
            },
        })
    } else {
        serde_json::json!({
            "id": "buildings",
            "type": "fill",
            "source": source,
            "source-layer": "buildings",
            "paint": { "fill-color": css_color(palette.building) },
        })
    });

    if options.political_view.is_some() {
        layers.push(serde_json::json!({
            "id": "borders",
            "type": "line",
            "source": source,
            "source-layer": "borders",
            "paint": { "line-color": css_color(palette.label), "line-width": 1 },
        }));
    }

    if features.contains(&"Hillshade") {
        layers.push(serde_json::json!({
            "id": "hillshade",
            "type": "fill",
            "source": source,
            "source-layer": "hillshade",
            "paint": { "fill-color": css_color(palette.graticule), "fill-opacity": 0.4 },
        }));
    }

    if let Some(density) = options.contour_density.as_deref() {
        let width = match density {
            "High" => 1.5,
            "Medium" => 1.0,
            _ => 0.5,
        };
        layers.push(serde_json::json!({
            "id": "contours",
            "type": "line",
            "source": source,
            "source-layer": "contour_lines",
            "paint": { "line-color": css_color(palette.graticule), "line-width": width },
        }));
    }

    for mode in &options.travel_modes {
        let Some(layer) = additional_feature_layer(match mode.as_str() {
            "Transit" => "Transit",
            "Truck" => "Logistics",
            other => other,
        }) else {
            continue;
        };
        layers.push(serde_json::json!({
            "id": format!("travel-mode-{}", mode.to_lowercase()),
            "type": "line",
            "source": source,
            "source-layer": layer,
            "paint": { "line-color": css_color(palette.marker), "line-width": 2 },
        }));
    }

    // `Off` is the one POI density that removes the symbol layer; every other
    // value scales how far apart its labels are placed.
    if options.poi_density.as_deref() != Some("Off") {
        let spacing = match options.poi_density.as_deref() {
            Some("VerySparse") => 500,
            Some("Sparse") => 350,
            Some("Dense") => 150,
            Some("VeryDense") => 80,
            _ => 250,
        };
        let mut places = serde_json::json!({
            "id": "places",
            "type": "symbol",
            "source": source,
            "source-layer": "places",
            "layout": {
                "icon-image": "poi-dot",
                "symbol-spacing": spacing,
                "text-field": ["get", "name"],
                "text-font": record.font_stack.clone(),
                "text-size": 12,
            },
            "paint": { "text-color": css_color(palette.label) },
        });
        if !options.poi_categories.is_empty() {
            places["filter"] = serde_json::json!([
                "in",
                ["get", "category"],
                ["literal", options.poi_categories.clone()],
            ]);
        }
        layers.push(places);
    }

    layers
}

/// Serialize a JSON document built in memory.
///
/// Infallible in practice: the sink is a `Vec`, and a `serde_json::Value` has
/// no unserializable state (its numbers are always finite and its keys always
/// strings).
fn to_json_bytes(value: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec_pretty(value).expect("serializing an in-memory JSON document")
}

// ---------------------------------------------------------------------------
// GetTile
// ---------------------------------------------------------------------------

/// A tile address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileCoord {
    pub z: u32,
    pub x: u32,
    pub y: u32,
}

impl TileCoord {
    /// The number of tiles per axis at this zoom: `2^z`.
    pub fn axis_tiles(zoom: u32) -> u64 {
        1_u64 << zoom.min(31)
    }

    /// Normalized Web Mercator position of a pixel inside this tile.
    fn pixel_position(self, column: u32, row: u32) -> Position {
        let axis = (Self::axis_tiles(self.z) * u64::from(TILE_SIZE)) as f64;
        let nx = (f64::from(self.x) * f64::from(TILE_SIZE) + f64::from(column)) / axis;
        let ny = (f64::from(self.y) * f64::from(TILE_SIZE) + f64::from(row)) / axis;
        (inv_merc_x(nx), inv_merc_y(ny))
    }
}

/// The bytes of one tile of `tileset`, in that tileset's own format.
pub fn tile_bytes(tileset: &TilesetRecord, coord: TileCoord, additional: &[String]) -> Vec<u8> {
    match tileset.format {
        TileFormat::VectorMvt => vector_tile_bytes(tileset, coord, additional),
        TileFormat::RasterJpeg => raster_tile_bytes(tileset, coord),
        TileFormat::RasterDemPng => dem_tile_bytes(coord),
    }
}

/// The layer extent every vector tile here is encoded at (the specification's
/// own default).
const TILE_EXTENT: u32 = 4096;

/// A vector tile: one layer per name the tileset record carries, plus one per
/// requested `AdditionalFeatures` value, each carrying geometry and attributes
/// derived from the tile's own address.
fn vector_tile_bytes(tileset: &TilesetRecord, coord: TileCoord, additional: &[String]) -> Vec<u8> {
    let mut names: Vec<String> = tileset.layers.clone();
    for feature in additional {
        if let Some(layer) = additional_feature_layer(feature) {
            if !names.iter().any(|name| name == &layer) {
                names.push(layer);
            }
        }
    }

    let extent = i32::try_from(TILE_EXTENT).unwrap_or(4096);
    let mut layers = Vec::with_capacity(names.len());
    for (index, name) in names.iter().enumerate() {
        let mut builder = LayerBuilder::new(name, TILE_EXTENT);
        let step = i32::try_from(index).unwrap_or(0) + 1;
        let margin = (step * 128).min(extent / 2 - 64);
        let attributes = |role: &'static str| {
            vec![
                ("layer", TagValue::Str(name.clone())),
                ("tileset", TagValue::Str(tileset.id.clone())),
                ("role", TagValue::Str(role.to_string())),
                ("z", TagValue::Int(i64::from(coord.z))),
                ("x", TagValue::Int(i64::from(coord.x))),
                ("y", TagValue::Int(i64::from(coord.y))),
                (
                    "name",
                    TagValue::Str(format!("{name} {}/{}/{}", coord.z, coord.x, coord.y)),
                ),
                ("height", TagValue::Int(i64::from(12 + coord.z))),
            ]
        };

        // A ring covering the tile, inset per layer so the layers of one tile
        // are distinguishable.
        let mut ring = GeometryWriter::new();
        ring.move_to(margin, margin);
        ring.line_to(&[
            (extent - margin, margin),
            (extent - margin, extent - margin),
            (margin, extent - margin),
        ]);
        ring.close_path();
        builder.add_feature(
            feature_id(coord, index, 0),
            GeomType::Polygon,
            ring.finish(),
            &attributes("area"),
        );

        // A line across the tile.
        let mut line = GeometryWriter::new();
        line.move_to(margin, extent / 2);
        line.line_to(&[(extent / 2, extent / 2 - margin), (extent - margin, extent / 2)]);
        builder.add_feature(
            feature_id(coord, index, 1),
            GeomType::LineString,
            line.finish(),
            &attributes("way"),
        );

        // A point at the tile's centre.
        let mut point = GeometryWriter::new();
        point.move_to(extent / 2, extent / 2);
        let mut point_attributes = attributes("point");
        point_attributes.push((
            "category",
            // The `category` attribute of a `places` feature, so a style's
            // category filter has something to filter on. Drawn from the
            // GENERATED `PoiCategory` list, not a copy of it, so a category the
            // model adds appears in tiles without this file being edited.
            TagValue::Str(POI_CATEGORY_VALUES[index % POI_CATEGORY_VALUES.len()].to_string()),
        ));
        builder.add_feature(
            feature_id(coord, index, 2),
            GeomType::Point,
            point.finish(),
            &point_attributes,
        );

        layers.push(builder.finish());
    }

    Tile { layers }.encode_to_vec()
}

/// A feature id that is unique within a tile and stable across requests.
fn feature_id(coord: TileCoord, layer: usize, slot: u64) -> u64 {
    (u64::from(coord.z) << 56)
        ^ (u64::from(coord.x) << 32)
        ^ (u64::from(coord.y) << 8)
        ^ (layer as u64 * 8 + slot)
}

/// A raster tile: the terrain field sampled over the tile's own pixel grid, so
/// the imagery really is a picture of this tile's ground.
fn raster_tile_bytes(tileset: &TilesetRecord, coord: TileCoord) -> Vec<u8> {
    let mut canvas = Canvas::new(TILE_SIZE, TILE_SIZE, tileset.palette.land);
    for row in 0..TILE_SIZE {
        for column in 0..TILE_SIZE {
            let (lon, lat) = coord.pixel_position(column, row);
            let color = terrain_color(&tileset.palette, elevation_at(lon, lat));
            canvas.put(i64::from(column), i64::from(row), color);
        }
    }
    // A one-pixel edge, which is what makes a tile seam visible at all.
    let last = i64::from(TILE_SIZE) - 1;
    canvas.hline(0, 0, last, tileset.palette.graticule);
    canvas.hline(last, 0, last, tileset.palette.graticule);
    canvas.vline(0, 0, last, tileset.palette.graticule);
    canvas.vline(last, 0, last, tileset.palette.graticule);
    canvas.to_jpeg()
}

/// A terrain tile: every pixel is the elevation at that pixel's position, in
/// the Terrarium encoding.
fn dem_tile_bytes(coord: TileCoord) -> Vec<u8> {
    let mut canvas = Canvas::new(TILE_SIZE, TILE_SIZE, [0, 0, 0, 0xff]);
    for row in 0..TILE_SIZE {
        for column in 0..TILE_SIZE {
            let (lon, lat) = coord.pixel_position(column, row);
            canvas.put(
                i64::from(column),
                i64::from(row),
                terrarium(elevation_at(lon, lat)),
            );
        }
    }
    canvas.to_png()
}

/// The elevation a Terrarium-encoded pixel carries.
pub fn decode_terrarium(pixel: Rgba) -> f64 {
    f64::from(pixel[0]) * 256.0 + f64::from(pixel[1]) + f64::from(pixel[2]) / 256.0 - 32768.0
}

// ---------------------------------------------------------------------------
// GetSprites
// ---------------------------------------------------------------------------

/// Where one icon sits on a rendered sheet.
#[derive(Debug, Clone)]
pub struct SpriteSlot {
    pub name: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub tint: Rgba,
    pub shape: IconShape,
}

/// A laid-out sheet: the slots, and the sheet's own size.
#[derive(Debug, Clone)]
pub struct SpriteLayout {
    pub slots: Vec<SpriteSlot>,
    pub width: u32,
    pub height: u32,
    pub pixel_ratio: u32,
}

/// Lay a sheet's icons out in a fixed grid, at `pixel_ratio` device pixels per
/// style pixel. The layout is the single source for both the PNG and the JSON
/// index, which is why the index always describes the sheet it is paired with.
pub fn layout_sprites(sheet: &SpriteSheetRecord, pixel_ratio: u32) -> SpriteLayout {
    let ratio = pixel_ratio.max(1);
    let columns = sheet.columns.max(1);
    let largest = sheet.icons.iter().map(|icon| icon.size).max().unwrap_or(16);
    let cell = (largest + sheet.gutter * 2) * ratio;
    let rows = (sheet.icons.len() as u32).div_ceil(columns).max(1);
    let slots = sheet
        .icons
        .iter()
        .enumerate()
        .map(|(index, icon)| {
            let column = index as u32 % columns;
            let row = index as u32 / columns;
            let size = icon.size * ratio;
            SpriteSlot {
                name: icon.name.clone(),
                x: column * cell + (cell - size) / 2,
                y: row * cell + (cell - size) / 2,
                width: size,
                height: size,
                tint: icon.tint,
                shape: icon.shape,
            }
        })
        .collect();
    SpriteLayout {
        slots,
        width: columns * cell,
        height: rows * cell,
        pixel_ratio: ratio,
    }
}

/// Draw a laid-out sheet as a transparent-background PNG.
pub fn render_sprite_sheet(layout: &SpriteLayout) -> Vec<u8> {
    let mut canvas = Canvas::new(layout.width, layout.height, [0, 0, 0, 0]);
    for slot in &layout.slots {
        let x = i64::from(slot.x);
        let y = i64::from(slot.y);
        let w = i64::from(slot.width);
        let h = i64::from(slot.height);
        match slot.shape {
            IconShape::Square => canvas.fill_rect(x, y, slot.width, slot.height, slot.tint),
            IconShape::Disc => {
                canvas.disc(x + w / 2, y + h / 2, slot.width / 2, slot.tint);
            }
            IconShape::Pin => {
                // A map pin: a head, and a stem down the middle to the point.
                canvas.disc(x + w / 2, y + h / 3, slot.width / 3, slot.tint);
                let stem = (slot.width / 4).max(1);
                canvas.fill_rect(
                    x + w / 2 - i64::from(stem) / 2,
                    y + h / 3,
                    stem,
                    slot.height * 2 / 3,
                    slot.tint,
                );
            }
        }
    }
    canvas.to_png()
}

/// The sheet's JSON index, in the MapLibre sprite-index format: one entry per
/// icon, naming the rectangle it occupies in the paired PNG.
pub fn sprite_index_json(layout: &SpriteLayout) -> Vec<u8> {
    let mut index = serde_json::Map::new();
    for slot in &layout.slots {
        index.insert(
            slot.name.clone(),
            serde_json::json!({
                "x": slot.x,
                "y": slot.y,
                "width": slot.width,
                "height": slot.height,
                "pixelRatio": layout.pixel_ratio,
                "sdf": false,
            }),
        );
    }
    to_json_bytes(&serde_json::Value::Object(index))
}

/// The media type and `pixelRatio` of a sprite file, from its name.
///
/// Matched against the exact set the modeled
/// `GetSpritesRequest$FileName` pattern (`^sprites(@2x)?\.(png|json)$`) admits,
/// rather than by looking at the extension alone, so a name the model refuses
/// is refused here too.
pub fn sprite_file(file_name: &str) -> Option<(&'static str, u32)> {
    match file_name {
        "sprites.png" => Some(("image/png", 1)),
        "sprites@2x.png" => Some(("image/png", 2)),
        "sprites.json" => Some(("application/json", 1)),
        "sprites@2x.json" => Some(("application/json", 2)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// GetGlyphs
// ---------------------------------------------------------------------------

/// The `.pbf` bytes of one glyph range of one font stack.
pub fn glyph_range_bytes(font: &FontRecord, stack: &str, start: u32, end: u32) -> Vec<u8> {
    super::glyph_pbf::glyph_range(font, stack, start, end).encode_to_vec()
}

// ---------------------------------------------------------------------------
// GetStaticMap
// ---------------------------------------------------------------------------

/// A shape an overlay asks to be drawn on a static map.
#[derive(Debug, Clone, PartialEq)]
pub enum OverlayShape {
    Point(Position),
    Line(Vec<Position>),
    Polygon(Vec<Position>),
}

/// Everything a static-map render needs, once the request has been validated.
#[derive(Debug, Clone)]
pub struct StaticMapView {
    /// Device pixels per style pixel (`map` is 1, `map@2x` is 2).
    pub pixel_ratio: u32,
    /// The requested image size in style pixels.
    pub width: u32,
    pub height: u32,
    /// Style pixels kept clear around the framed area.
    pub padding: u32,
    pub envelope: Envelope,
    /// The requested centre, when the request framed the image by centre.
    pub center: Option<Position>,
    /// Positions the request asked to be contained.
    pub markers: Vec<Position>,
    /// 1 for `Small`, 2 for `Large`: how big markers and the scale bar are
    /// drawn.
    pub label_scale: u32,
    pub show_pois: bool,
    /// Whether the scale bar's leading unit is miles rather than kilometres.
    pub scale_bar_miles: bool,
    pub overlay: Vec<OverlayShape>,
}

impl StaticMapView {
    /// The image size in device pixels.
    pub fn pixel_size(&self) -> (u32, u32) {
        (self.width * self.pixel_ratio, self.height * self.pixel_ratio)
    }
}

/// Render a static map: the terrain field inside the framed envelope, a
/// graticule, the requested markers and overlay, and a scale bar.
pub fn render_static_map(palette: &Palette, view: &StaticMapView) -> Vec<u8> {
    let (pixel_width, pixel_height) = view.pixel_size();
    let mut canvas = Canvas::new(pixel_width, pixel_height, palette.land);
    let inset = view.padding * view.pixel_ratio;
    let frame_width = pixel_width.saturating_sub(inset * 2).max(1);
    let frame_height = pixel_height.saturating_sub(inset * 2).max(1);

    // The map surface, sampled from the shared terrain field. Elevation is
    // evaluated once per block rather than per pixel: a 1400x1400@2x image is
    // 7.8M pixels, and a block is far finer than any feature of the field.
    let block = (4 * view.pixel_ratio).max(1);
    for row in (0..frame_height).step_by(block as usize) {
        for column in (0..frame_width).step_by(block as usize) {
            let fx = (f64::from(column) + f64::from(block) / 2.0) / f64::from(frame_width);
            let fy = (f64::from(row) + f64::from(block) / 2.0) / f64::from(frame_height);
            let (lon, lat) = view.envelope.unproject_fraction(fx, fy);
            let color = terrain_color(palette, elevation_at(lon, lat));
            canvas.fill_rect(
                i64::from(inset + column),
                i64::from(inset + row),
                block,
                block,
                color,
            );
        }
    }

    draw_graticule(&mut canvas, palette, view, (frame_width, frame_height));

    let project = |position: Position| {
        let (px, py) = view
            .envelope
            .project(position, frame_width, frame_height);
        (
            px.saturating_add(i64::from(inset)),
            py.saturating_add(i64::from(inset)),
        )
    };

    for shape in &view.overlay {
        draw_overlay_shape(&mut canvas, palette, shape, &project);
    }

    if view.show_pois {
        // POIs are placed on a fixed lattice over the view and kept where the
        // terrain field says there is land, so they land on ground rather than
        // in the sea.
        for ix in 0..5 {
            for iy in 0..5 {
                let fx = (f64::from(ix) + 0.5) / 5.0;
                let fy = (f64::from(iy) + 0.5) / 5.0;
                let position = view.envelope.unproject_fraction(fx, fy);
                if elevation_at(position.0, position.1) < 0.0 {
                    continue;
                }
                let (px, py) = project(position);
                canvas.disc(px, py, 2 * view.pixel_ratio, palette.label);
            }
        }
    }

    for marker in &view.markers {
        let (px, py) = project(*marker);
        canvas.disc(px, py, 3 * view.label_scale * view.pixel_ratio, palette.marker);
    }

    if let Some(center) = view.center {
        let (px, py) = project(center);
        let radius = 5 * view.label_scale * view.pixel_ratio;
        canvas.disc(px, py, radius, palette.marker);
        canvas.hline(
            py,
            px.saturating_sub(i64::from(radius)),
            px.saturating_add(i64::from(radius)),
            palette.label,
        );
        canvas.vline(
            px,
            py.saturating_sub(i64::from(radius)),
            py.saturating_add(i64::from(radius)),
            palette.label,
        );
    }

    draw_scale_bar(&mut canvas, palette, view, frame_width);

    // The frame edge, so the padded border is visibly a border.
    let right = i64::from(pixel_width) - 1 - i64::from(inset);
    let bottom = i64::from(pixel_height) - 1 - i64::from(inset);
    canvas.hline(i64::from(inset), i64::from(inset), right, palette.graticule);
    canvas.hline(bottom, i64::from(inset), right, palette.graticule);
    canvas.vline(i64::from(inset), i64::from(inset), bottom, palette.graticule);
    canvas.vline(right, i64::from(inset), bottom, palette.graticule);

    canvas.to_jpeg()
}

/// Draw meridians and parallels at a round interval chosen for the view's span.
fn draw_graticule(
    canvas: &mut Canvas,
    palette: &Palette,
    view: &StaticMapView,
    frame: (u32, u32),
) {
    let (frame_width, frame_height) = frame;
    let inset = i64::from(view.padding * view.pixel_ratio);
    let envelope = view.envelope;
    let step = graticule_step(envelope.max_lon - envelope.min_lon);
    let mut lon = (envelope.min_lon / step).ceil() * step;
    while lon <= envelope.max_lon {
        let (px, _) = envelope.project((lon, envelope.center().1), frame_width, frame_height);
        canvas.vline(
            px + inset,
            inset,
            inset + i64::from(frame_height) - 1,
            palette.graticule,
        );
        lon += step;
    }
    let step = graticule_step(envelope.max_lat - envelope.min_lat);
    let mut lat = (envelope.min_lat / step).ceil() * step;
    while lat <= envelope.max_lat {
        let (_, py) = envelope.project((envelope.center().0, lat), frame_width, frame_height);
        canvas.hline(
            py + inset,
            inset,
            inset + i64::from(frame_width) - 1,
            palette.graticule,
        );
        lat += step;
    }
}

/// The largest round interval, from a 1-2-5 ladder of degrees, that puts at
/// most twelve lines across `span` degrees.
fn graticule_step(span: f64) -> f64 {
    const LADDER: [f64; 15] = [
        30.0, 10.0, 5.0, 2.0, 1.0, 0.5, 0.2, 0.1, 0.05, 0.02, 0.01, 0.005, 0.002, 0.001, 0.0005,
    ];
    let span = span.abs().max(1e-9);
    for step in LADDER {
        if span / step >= 2.0 {
            return step;
        }
    }
    LADDER[LADDER.len() - 1]
}

fn draw_overlay_shape(
    canvas: &mut Canvas,
    palette: &Palette,
    shape: &OverlayShape,
    project: &impl Fn(Position) -> (i64, i64),
) {
    match shape {
        OverlayShape::Point(position) => {
            let (px, py) = project(*position);
            canvas.disc(px, py, 4, palette.marker);
        }
        OverlayShape::Line(points) => {
            for pair in points.windows(2) {
                canvas.line(project(pair[0]), project(pair[1]), palette.marker);
            }
        }
        OverlayShape::Polygon(points) => {
            for pair in points.windows(2) {
                canvas.line(project(pair[0]), project(pair[1]), palette.marker);
            }
            if let (Some(first), Some(last)) = (points.first(), points.last()) {
                canvas.line(project(*last), project(*first), palette.marker);
            }
        }
    }
}

/// Draw the scale bar: a round ground distance, measured at the view's centre
/// latitude, in the unit the request asked for.
fn draw_scale_bar(canvas: &mut Canvas, palette: &Palette, view: &StaticMapView, frame_width: u32) {
    let metres_per_pixel = view.envelope.metres_per_pixel(frame_width);
    if !metres_per_pixel.is_finite() || metres_per_pixel <= 0.0 {
        return;
    }
    let target_metres = metres_per_pixel * f64::from(frame_width) / 4.0;
    let metres = if view.scale_bar_miles {
        // 1 international mile is exactly 1609.344 m.
        round_ladder(target_metres / 1609.344) * 1609.344
    } else {
        round_ladder(target_metres / 1000.0) * 1000.0
    };
    let length = (metres / metres_per_pixel).round().max(1.0);
    let Ok(length) = u32::try_from(length as i64) else {
        return;
    };
    let length = length.min(frame_width.saturating_sub(1)).max(1);

    let inset = i64::from(view.padding * view.pixel_ratio);
    let thickness = 2 * view.label_scale * view.pixel_ratio;
    let x = inset + i64::from(8 * view.pixel_ratio);
    let y = i64::from(canvas.height()) - inset - i64::from(8 * view.pixel_ratio);
    canvas.fill_rect(x, y, length, thickness, palette.label);
    let tick = i64::from(4 * view.label_scale * view.pixel_ratio);
    canvas.vline(x, y - tick, y + i64::from(thickness), palette.label);
    canvas.vline(
        x + i64::from(length),
        y - tick,
        y + i64::from(thickness),
        palette.label,
    );
}

/// `value` rounded down onto the 1-2-5 ladder a map scale bar uses.
fn round_ladder(value: f64) -> f64 {
    if !value.is_finite() || value <= 0.0 {
        return 1.0;
    }
    let magnitude = 10_f64.powf(value.log10().floor());
    let normalized = value / magnitude;
    let step = if normalized >= 5.0 {
        5.0
    } else if normalized >= 2.0 {
        2.0
    } else {
        1.0
    };
    step * magnitude
}

// ---------------------------------------------------------------------------
// Request parsing shared by GetStaticMap's position parameters
// ---------------------------------------------------------------------------

/// Parse a comma-separated `longitude,latitude[,longitude,latitude...]` list.
///
/// The modeled pattern already admits only digits, signs and commas, so what is
/// checked here is what the pattern cannot express: that the values come in
/// pairs, that there are as many pairs as the member documents, and that each
/// pair is a position on Earth (WGS 84 bounds). Every refusal names the member
/// and the constraint but not the value — these members target `@sensitive`
/// shapes, so a caller's coordinates do not belong in an error message.
pub fn parse_positions(
    field: &str,
    raw: &str,
    minimum_pairs: usize,
    maximum_pairs: Option<usize>,
) -> Result<Vec<Position>, FieldFault> {
    let too_many = |maximum: usize| {
        FieldFault::new(
            field,
            "FieldValidationFailed",
            format!("{field} takes at most {maximum} longitude,latitude pair(s)"),
        )
    };
    let mut numbers = Vec::new();
    for part in raw.split(',') {
        // Refused as soon as the list runs past what the member accepts, so a
        // long value is never materialised in full just to be rejected.
        if let Some(maximum) = maximum_pairs {
            if numbers.len() >= maximum * 2 {
                return Err(too_many(maximum));
            }
        }
        let Ok(value) = part.trim().parse::<f64>() else {
            return Err(FieldFault::new(
                field,
                "CannotParse",
                format!("{field} must be a comma-separated list of numbers"),
            ));
        };
        if !value.is_finite() {
            return Err(FieldFault::new(
                field,
                "CannotParse",
                format!("{field} must be a comma-separated list of finite numbers"),
            ));
        }
        numbers.push(value);
    }
    if numbers.len() % 2 != 0 {
        return Err(FieldFault::new(
            field,
            "FieldValidationFailed",
            format!("{field} must hold complete longitude,latitude pairs"),
        ));
    }
    let positions: Vec<Position> = numbers.chunks(2).map(|pair| (pair[0], pair[1])).collect();
    if positions.len() < minimum_pairs {
        return Err(FieldFault::new(
            field,
            "FieldValidationFailed",
            format!("{field} requires at least {minimum_pairs} longitude,latitude pair(s)"),
        ));
    }
    for (lon, lat) in &positions {
        check_position(field, *lon, *lat)?;
    }
    Ok(positions)
}

/// WGS 84 bounds: longitude in `[-180, 180]`, latitude in `[-90, 90]`.
fn check_position(field: &str, lon: f64, lat: f64) -> Result<(), FieldFault> {
    if !(-180.0..=180.0).contains(&lon) {
        return Err(FieldFault::new(
            field,
            "FieldValidationFailed",
            format!("{field} longitude must be within [-180, 180]"),
        ));
    }
    if !(-90.0..=90.0).contains(&lat) {
        return Err(FieldFault::new(
            field,
            "FieldValidationFailed",
            format!("{field} latitude must be within [-90, 90]"),
        ));
    }
    Ok(())
}

/// The deepest a GeoJSON overlay may nest before it is refused.
///
/// RFC 7946 nests at most three levels of arrays inside a `MultiPolygon`, and a
/// `GeometryCollection` or `FeatureCollection` adds one level per container, so
/// this is far above any real overlay; it exists so the traversal's depth is a
/// property of this module rather than of the JSON parser's own limit.
pub const MAX_OVERLAY_DEPTH: u32 = 16;

/// The most shapes one overlay may draw. The modeled `GeoJsonOverlay` length
/// maximum of 4200 characters already bounds this (the shortest drawable
/// geometry is tens of characters), so it is a backstop on the render work.
pub const MAX_OVERLAY_SHAPES: usize = 256;

/// Parse a GeoJSON overlay into the shapes to draw.
///
/// Accepts the GeoJSON object kinds RFC 7946 defines — a `FeatureCollection`, a
/// `Feature`, or a bare geometry, including the `Multi*` and
/// `GeometryCollection` forms — and refuses anything that is not GeoJSON, which
/// is a `CannotParse` validation failure rather than a silently empty overlay.
pub fn parse_geojson_overlay(raw: &str) -> Result<Vec<OverlayShape>, FieldFault> {
    let field = "GeoJsonOverlay";
    let document: serde_json::Value = serde_json::from_str(raw).map_err(|error| {
        FieldFault::new(
            field,
            "CannotParse",
            format!("{field} must be a GeoJSON document: {error}"),
        )
    })?;
    let mut shapes = Vec::new();
    collect_geojson(field, &document, 0, &mut shapes)?;
    if shapes.is_empty() {
        return Err(FieldFault::new(
            field,
            "FieldValidationFailed",
            format!("{field} holds no drawable geometry"),
        ));
    }
    Ok(shapes)
}

fn collect_geojson(
    field: &str,
    node: &serde_json::Value,
    depth: u32,
    shapes: &mut Vec<OverlayShape>,
) -> Result<(), FieldFault> {
    if depth > MAX_OVERLAY_DEPTH {
        return Err(FieldFault::new(
            field,
            "FieldValidationFailed",
            format!("{field} nests more than {MAX_OVERLAY_DEPTH} levels deep"),
        ));
    }
    if shapes.len() > MAX_OVERLAY_SHAPES {
        return Err(too_many_shapes(field));
    }
    let kind = node.get("type").and_then(serde_json::Value::as_str);
    let Some(kind) = kind else {
        return Err(FieldFault::new(
            field,
            "CannotParse",
            format!("{field} object is missing its `type` member"),
        ));
    };
    match kind {
        "FeatureCollection" => {
            let features = node
                .get("features")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    FieldFault::new(
                        field,
                        "CannotParse",
                        format!("{field} FeatureCollection is missing its `features` array"),
                    )
                })?;
            for feature in features {
                collect_geojson(field, feature, depth + 1, shapes)?;
            }
        }
        "Feature" => {
            if let Some(geometry) = node.get("geometry") {
                if !geometry.is_null() {
                    collect_geojson(field, geometry, depth + 1, shapes)?;
                }
            }
        }
        "GeometryCollection" => {
            let geometries = node
                .get("geometries")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    FieldFault::new(
                        field,
                        "CannotParse",
                        format!("{field} GeometryCollection is missing its `geometries` array"),
                    )
                })?;
            for geometry in geometries {
                collect_geojson(field, geometry, depth + 1, shapes)?;
            }
        }
        "Point" => push_shape(
            field,
            OverlayShape::Point(geojson_position(field, node.get("coordinates"))?),
            shapes,
        )?,
        "MultiPoint" => {
            for position in geojson_positions(field, node.get("coordinates"))? {
                push_shape(field, OverlayShape::Point(position), shapes)?;
            }
        }
        "LineString" => push_shape(
            field,
            OverlayShape::Line(geojson_positions(field, node.get("coordinates"))?),
            shapes,
        )?,
        "MultiLineString" | "Polygon" => {
            let rings = node
                .get("coordinates")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| coordinates_fault(field))?;
            for ring in rings {
                let positions = geojson_positions(field, Some(ring))?;
                let shape = if kind == "Polygon" {
                    OverlayShape::Polygon(positions)
                } else {
                    OverlayShape::Line(positions)
                };
                push_shape(field, shape, shapes)?;
            }
        }
        "MultiPolygon" => {
            let polygons = node
                .get("coordinates")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| coordinates_fault(field))?;
            for polygon in polygons {
                let rings = polygon
                    .as_array()
                    .ok_or_else(|| coordinates_fault(field))?;
                for ring in rings {
                    let positions = geojson_positions(field, Some(ring))?;
                    push_shape(field, OverlayShape::Polygon(positions), shapes)?;
                }
            }
        }
        other => {
            return Err(FieldFault::new(
                field,
                "CannotParse",
                format!("{field} carries an unknown GeoJSON type `{other}`"),
            ))
        }
    }
    Ok(())
}

fn coordinates_fault(field: &str) -> FieldFault {
    FieldFault::new(
        field,
        "CannotParse",
        format!("{field} geometry is missing well-formed `coordinates`"),
    )
}

fn too_many_shapes(field: &str) -> FieldFault {
    FieldFault::new(
        field,
        "FieldValidationFailed",
        format!("{field} draws more than {MAX_OVERLAY_SHAPES} shapes"),
    )
}

/// Append one shape, enforcing [`MAX_OVERLAY_SHAPES`] at the point the shape is
/// added rather than only on entry to a node, so a single `Multi*` geometry
/// cannot push past the cap inside one call.
fn push_shape(
    field: &str,
    shape: OverlayShape,
    shapes: &mut Vec<OverlayShape>,
) -> Result<(), FieldFault> {
    if shapes.len() >= MAX_OVERLAY_SHAPES {
        return Err(too_many_shapes(field));
    }
    shapes.push(shape);
    Ok(())
}

fn geojson_position(
    field: &str,
    node: Option<&serde_json::Value>,
) -> Result<Position, FieldFault> {
    let pair = node
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| coordinates_fault(field))?;
    let (Some(lon), Some(lat)) = (
        pair.first().and_then(serde_json::Value::as_f64),
        pair.get(1).and_then(serde_json::Value::as_f64),
    ) else {
        return Err(coordinates_fault(field));
    };
    check_position(field, lon, lat)?;
    Ok((lon, lat))
}

fn geojson_positions(
    field: &str,
    node: Option<&serde_json::Value>,
) -> Result<Vec<Position>, FieldFault> {
    let list = node
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| coordinates_fault(field))?;
    let mut positions = Vec::with_capacity(list.len());
    for entry in list {
        positions.push(geojson_position(field, Some(entry))?);
    }
    if positions.is_empty() {
        return Err(coordinates_fault(field));
    }
    Ok(positions)
}

/// The style a static map is drawn in: `StaticMapStyle` names a subset of
/// `MapStyle`, so the two share one palette catalog.
///
/// Which names are admissible is decided by the model and nothing else:
/// `STATIC_MAP_STYLE_VALUES` is the generated list of `StaticMapStyle`'s
/// members — the same list `GetStaticMap`'s refusal message quotes — so a value
/// the model adds later is admitted here (as long as `MapStyle` names it too,
/// which is what gives it a palette) without this function being edited.
pub fn static_map_style(value: Option<&str>) -> Option<MapStyleKind> {
    match value {
        None => Some(MapStyleKind::Standard),
        Some(name) => {
            if !STATIC_MAP_STYLE_VALUES.contains(&name) {
                return None;
            }
            MapStyleKind::from_wire(name)
        }
    }
}
