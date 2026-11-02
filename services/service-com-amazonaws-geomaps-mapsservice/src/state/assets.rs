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

use prost::Message as _;
use sha2::{Digest as _, Sha256};

use super::canvas::Canvas;
use super::vector_tile::{GeomType, GeometryWriter, LayerBuilder, TagValue, Tile};
// The generated list of `TileAdditionalFeature`'s members: the ONE place this
// crate decides which strings that enum admits.
use crate::ops::types::{
    POI_CATEGORY_VALUES, STATIC_MAP_STYLE_VALUES, TILE_ADDITIONAL_FEATURE_VALUES,
};
use super::style_layers::{self, LayerGate, LayerRole, StyleLayerSpec};
use super::{
    boundaries, labels, ColorSchemeKind, ComAmazonawsGeomapsMapsserviceState, FontRecord, Hsla,
    IconShape, MapStyleKind, PaintValue, Palette, Rgba, SpriteSheetRecord, StyleRecord, TileFormat,
    TilesetRecord, VariantKind, ADMIN_LAYER, BUILDING_LAYER, GROUND_LAYER, POI_LAYER, ROAD_LAYER,
    TRAFFIC_FLOW_LAYER, TRAFFIC_INCIDENT_LAYER, VECTOR_MAX_ZOOM, WATER_LAYER,
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
        return palette.water.rgba();
    }
    let factor = (elevation / 2000.0).clamp(0.0, 1.0);
    blend(palette.land.rgba(), palette.building.rgba(), factor)
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

/// A style-document paint value, in the JSON a MapLibre paint property takes:
/// a colour literal for a flat colour, and an `interpolate` EXPRESSION for a
/// colour that changes with zoom.
///
/// An expression is an array, so a serializer that could only write a string
/// here would hand a renderer a value it cannot vary — which is why the catalog
/// carries [`PaintValue`] rather than a colour.
fn paint_value(value: &PaintValue) -> serde_json::Value {
    match value {
        PaintValue::Flat(color) => color.css().into(),
        PaintValue::ZoomRamp(stops) => {
            let mut expression = vec![
                serde_json::json!("interpolate"),
                serde_json::json!(["linear"]),
                serde_json::json!(["zoom"]),
            ];
            for (zoom, color) in stops {
                expression.push(zoom_literal(*zoom));
                expression.push(color.css().into());
            }
            serde_json::Value::Array(expression)
        }
    }
}

/// A zoom level as a JSON number: an integer when it is one, so a stop at zoom
/// 16 is written `16` rather than `16.0`.
fn zoom_literal(zoom: f64) -> serde_json::Value {
    if zoom.fract() == 0.0 {
        serde_json::json!(zoom as i64)
    } else {
        serde_json::json!(zoom)
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

/// The attribute every vector-tile feature carries saying which KIND of
/// geometry it is, and the three values it takes. A style layer filters on it,
/// and [`vector_tile_bytes`] writes it, so the two cannot drift apart.
pub const ROLE_ATTRIBUTE: &str = "role";
pub const AREA_ROLE: &str = "area";
pub const WAY_ROLE: &str = "way";
pub const POINT_ROLE: &str = "point";

/// The attribute a boundary feature over a disputed territory carries, saying
/// which country's political view claims it. Written by [`vector_tile_bytes`]
/// and filtered on by the disputed-boundary style layers, so a request's
/// `PoliticalView` selects the alignment that view recognises.
pub const POLITICAL_VIEW_ATTRIBUTE: &str = "political_view";

/// The traffic tileset and the style-document source name it is declared
/// under. Which of its layers carry incident events is the tileset record's
/// own `incident_layers`, because `Traffic: Congestion` asks for the flow of
/// traffic without them.
const TRAFFIC_TILESET: &str = "vector.traffic";
const TRAFFIC_SOURCE: &str = "traffic";

/// The `TileAdditionalFeature` members whose layers a request knob changes the
/// drawing of: the contour lines a `ContourDensity` sets the weight of, and
/// the hillshade shading drawn over the ground.
const CONTOUR_FEATURE: &str = "ContourLines";
const HILLSHADE_FEATURE: &str = "Hillshade";

/// Whether `name` is the tile layer that the additional feature `feature`
/// adds. Asked through [`additional_feature_layer`] rather than against a
/// second spelling of the layer name, so the style layer and the tile layer
/// cannot drift apart.
fn is_feature_layer(feature: &str, name: &str) -> bool {
    additional_feature_layer(feature).as_deref() == Some(name)
}

/// The elevation tileset a `Terrain3D` request reads, and the source name it is
/// declared under.
const DEM_TILESET: &str = "raster.dem";
const TERRAIN_SOURCE: &str = "terrain";

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
        features.push(CONTOUR_FEATURE);
    }
    if options.terrain.as_deref() == Some(HILLSHADE_FEATURE) {
        features.push(HILLSHADE_FEATURE);
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
    // Which tile layers each declared source really serves, in the order the
    // document declares the sources. A style layer is drawn only when the tiles
    // behind it carry the layer it reads, so this list is what switches the
    // traffic, contour, hillshade and travel-mode families on: asking for one
    // of those knobs is what puts its layer in the tiles the template fetches.
    let mut available: Vec<(String, Vec<String>)> = Vec::new();
    // The ground is painted before anything else and has no geometry of its
    // own, which is exactly what a MapLibre `background` layer is: the
    // basemap's ground layer IS this document's background, not a polygon fill
    // drawn over one.
    let mut layers: Vec<serde_json::Value> = vec![serde_json::json!({
        "id": GROUND_LAYER,
        "type": "background",
        "paint": { "background-color": paint_value(&record.background) },
    })];

    for source in &record.sources {
        let Some(tileset) = state.tilesets.get(&source.tileset) else {
            continue;
        };
        let mut url = absolute(&source.tile_url_template);
        let mut tile_layers = tileset.layers.clone();
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
            // Each requested feature is another layer in the tiles THIS
            // template fetches, so the document draws it.
            for feature in &features {
                if let Some(layer) = additional_feature_layer(feature) {
                    if !tile_layers.contains(&layer) {
                        tile_layers.push(layer);
                    }
                }
            }
        }
        sources.insert(source.name.clone(), source_entry(tileset, &url));
        match tileset.format {
            TileFormat::VectorMvt => {
                available.push((source.name.clone(), tile_layers));
            }
            TileFormat::RasterJpeg | TileFormat::RasterDemPng => layers.push(serde_json::json!({
                "id": format!("{}-imagery", source.name),
                "type": "raster",
                "source": source.name,
            })),
        }
    }

    // Traffic is its own tileset, so asking for it adds a source as well as
    // layers. `Congestion` asks for the flow of traffic only, which is every
    // layer the tileset does not declare an incident layer; `All` makes the
    // incident layers available too.
    if let (Some(traffic), Some(tileset)) = (
        options.traffic.as_deref(),
        state.tilesets.get(TRAFFIC_TILESET),
    ) {
        let url = absolute(&tile_url_template(&tileset.id));
        sources.insert(TRAFFIC_SOURCE.to_string(), source_entry(tileset, &url));
        let tile_layers: Vec<String> = tileset
            .layers
            .iter()
            .filter(|name| traffic == "All" || !tileset.incident_layers.contains(name))
            .cloned()
            .collect();
        available.push((TRAFFIC_SOURCE.to_string(), tile_layers));
    }

    layers.extend(published_layers(record, options, &available));
    // 3D terrain is elevation data, which is the DEM tileset rather than a
    // layer of the basemap.
    let mut terrain_property = None;
    if options.terrain.as_deref() == Some("Terrain3D") {
        if let Some(tileset) = state.tilesets.get(DEM_TILESET) {
            let url = absolute(&tile_url_template(&tileset.id));
            sources.insert(TERRAIN_SOURCE.to_string(), source_entry(tileset, &url));
            terrain_property = Some(serde_json::json!({
                "source": TERRAIN_SOURCE,
                "exaggeration": 1.0,
            }));
        }
    }

    // The metadata reports facts about the published DOCUMENT — the catalog
    // version it was published at and the virtual variants the request put in
    // effect — and nothing else: the style and the colour scheme it was built
    // for are its `id` and its `name`, and echoing the request's knobs back
    // would put members in the document that the published style does not
    // declare.
    let mut metadata = serde_json::Map::new();
    // The published release version, carried whole: the string the catalog
    // publishes, qualifier and all, is what the `id` ends with too.
    metadata.insert("version".to_string(), record.version.clone().into());
    metadata.insert("aws:virtual-variant".to_string(), virtual_variants());

    let mut document = serde_json::Map::new();
    document.insert("version".to_string(), 8.into());
    document.insert("id".to_string(), record.id.clone().into());
    document.insert("name".to_string(), record.name.clone().into());
    document.insert("created".to_string(), record.created.clone().into());
    document.insert("modified".to_string(), record.modified.clone().into());
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
    document.insert(
        "light".to_string(),
        serde_json::json!({
            "anchor": record.light.anchor,
            "intensity": record.light.intensity,
            "position": record.light.position,
        }),
    );
    document.insert(
        "sky".to_string(),
        serde_json::json!({
            "sky-color": record.sky.sky_color.css(),
            "sky-horizon-blend": record.sky.sky_horizon_blend,
            "horizon-color": record.sky.horizon_color.css(),
            "horizon-fog-blend": record.sky.horizon_fog_blend,
            "fog-color": record.sky.fog_color.css(),
            "fog-ground-blend": record.sky.fog_ground_blend,
            "atmosphere-blend": record.sky.atmosphere_blend,
        }),
    );
    document.insert(
        "projection".to_string(),
        serde_json::json!({ "type": record.projection }),
    );
    if let Some(terrain) = terrain_property {
        document.insert("terrain".to_string(), terrain);
    }
    document.insert("layers".to_string(), layers.into());

    to_json_bytes(&serde_json::Value::Object(document))
}

/// The virtual variants the published styles declare, by the request knob each
/// one varies, as the document's variant block lists them.
///
/// A VIRTUAL VARIANT is a rendering of the whole style a client can switch to —
/// the three-dimensional terrain rendering is one — as opposed to a detail
/// drawn from a tile layer added to the same rendering (the hillshade shading,
/// the contour lines, a travel mode's network), which is a layer of this style
/// and not another rendering of it.
///
/// The block is published content of the style: it states what the style HAS,
/// so a client reads it to discover which variants it may ask for. It therefore
/// does not vary with the request — a document that listed only the variant
/// this request already selected would tell a client nothing it did not
/// already know, and would make a feature it has not asked for yet look
/// unavailable.
const VIRTUAL_VARIANTS: &[(&str, &[&str])] = &[(TERRAIN_VARIANT_KEY, &[TERRAIN_3D_VARIANT])];

/// The request knob the published styles' virtual variants are keyed on, and
/// the id of the three-dimensional terrain rendering it names. `Terrain`'s
/// `Terrain3D` value is valid for every map style — the model restricts only
/// `Hillshade`, to `Standard` and `Monochrome` — so every published style
/// declares it.
const TERRAIN_VARIANT_KEY: &str = "terrain";
const TERRAIN_3D_VARIANT: &str = "3d_terrain";

/// The document's variant block: one entry per knob in [`VIRTUAL_VARIANTS`],
/// naming the knob and the variants published under it.
fn virtual_variants() -> serde_json::Value {
    let entries: Vec<serde_json::Value> = VIRTUAL_VARIANTS
        .iter()
        .map(|(key, variants)| {
            let listed: Vec<serde_json::Value> = variants
                .iter()
                .map(|id| serde_json::json!({ "id": id }))
                .collect();
            serde_json::json!({ "key": key, "variants": listed })
        })
        .collect();
    serde_json::Value::Array(entries)
}

/// A style-document source entry for one tileset: its type, its tile template,
/// the credit line its data is served under and the zoom range the tileset
/// actually has tiles for.
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
    entry.insert(
        "attribution".to_string(),
        tileset.attribution.clone().into(),
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

/// The layer list of a published style, projected through the request's knobs.
///
/// The list itself is catalog data ([`style_layers::BASEMAP_LAYERS`]); what this
/// adds is the three rules that decide whether and how one entry is drawn:
///
/// * a layer is drawn only when the tiles behind it CARRY the tile layer it
///   reads, which is what switches the traffic, contour, hillshade and
///   travel-mode families on and off, and what keeps a style that declares no
///   basemap source from declaring basemap layers;
/// * the knobs that select between layers of one tile layer are the layer's own
///   [`LayerGate`];
/// * everything else about the layer object — its type, the geometry role it
///   filters to, its colour, its width and its icon — is derived from the
///   entry's [`LayerRole`], so one rule paints every member of a family.
fn published_layers(
    record: &StyleRecord,
    options: &StyleOptions,
    available: &[(String, Vec<String>)],
) -> Vec<serde_json::Value> {
    let mut layers = Vec::with_capacity(record.layers.len());
    for spec in record.layers {
        if !gate_open(spec.gate, options) {
            continue;
        }
        // The first source, in the order the document declares them, whose
        // tiles carry this layer's tile layer. A layer no declared source
        // serves is not drawn, because there would be nothing behind it.
        let Some(source) = available
            .iter()
            .find(|(_, tile_layers)| tile_layers.iter().any(|name| name == spec.source_layer))
            .map(|(name, _)| name.clone())
        else {
            continue;
        };
        layers.push(layer_object(record, options, spec, &source));
    }
    layers
}

/// Whether the request leaves a gated layer switched on.
fn gate_open(gate: LayerGate, options: &StyleOptions) -> bool {
    match gate {
        LayerGate::Always => true,
        // `Off` is the one POI density that hides points of interest
        // altogether; every other value only changes how far apart they are
        // placed.
        LayerGate::Poi => options.poi_density.as_deref() != Some("Off"),
        // `Buildings3D` is the one value the model declares, and it is what
        // raises the footprints into extrusions.
        LayerGate::Buildings3D => options.buildings.as_deref() == Some("Buildings3D"),
    }
}

/// One catalog entry as a style-document layer object.
fn layer_object(
    record: &StyleRecord,
    options: &StyleOptions,
    spec: &StyleLayerSpec,
    source: &str,
) -> serde_json::Value {
    let palette = &record.palette;
    let color = layer_color(palette, spec);
    let mut layer = serde_json::Map::new();
    layer.insert("id".to_string(), spec.id.into());
    layer.insert("type".to_string(), spec.role.layer_type().into());
    layer.insert("source".to_string(), source.into());
    layer.insert("source-layer".to_string(), spec.source_layer.into());
    if spec.min_zoom > 0 {
        layer.insert("minzoom".to_string(), spec.min_zoom.into());
    }
    layer.insert("filter".to_string(), geometry_filter(spec, options));

    let mut paint = serde_json::Map::new();
    match spec.role {
        LayerRole::Area | LayerRole::Shading => {
            paint.insert("fill-color".to_string(), color.css().into());
        }
        LayerRole::Extrusion => {
            paint.insert("fill-extrusion-color".to_string(), color.css().into());
            paint.insert(
                "fill-extrusion-height".to_string(),
                serde_json::json!(["get", "height"]),
            );
        }
        LayerRole::Way | LayerRole::Casing | LayerRole::Shadow => {
            paint.insert("line-color".to_string(), color.css().into());
            paint.insert("line-width".to_string(), line_width_value(spec, options));
        }
        LayerRole::Label | LayerRole::Icon => {
            paint.insert("text-color".to_string(), palette.label.css().into());
            paint.insert("text-halo-color".to_string(), palette.land.css().into());
            paint.insert("text-halo-width".to_string(), LABEL_HALO_WIDTH.into());
        }
    }
    layer.insert("paint".to_string(), paint.into());

    if spec.role.is_symbol() {
        let mut layout = serde_json::Map::new();
        layout.insert(
            "text-field".to_string(),
            serde_json::json!(["get", "name"]),
        );
        layout.insert("text-font".to_string(), record.font_stack.clone().into());
        layout.insert("text-size".to_string(), LABEL_TEXT_SIZE.into());
        if spec.role == LayerRole::Icon {
            // A feature names its own icon where it has one; everything else
            // falls back to an icon the sheet this document points at carries.
            layout.insert(
                "icon-image".to_string(),
                serde_json::json!([
                    "coalesce",
                    ["image", ["get", "icon"]],
                    ["image", style_layers::fallback_icon(spec)],
                ]),
            );
        }
        if spec.gate == LayerGate::Poi {
            layout.insert(
                "symbol-spacing".to_string(),
                poi_spacing(options.poi_density.as_deref()).into(),
            );
        }
        layer.insert("layout".to_string(), layout.into());
    } else if let Some(layout) = line_layout(spec.role) {
        layer.insert("layout".to_string(), layout);
    }

    serde_json::Value::Object(layer)
}

/// The layout block a LINE layer declares, which is how the segments of one way
/// are joined where they meet.
///
/// A layer's layout is part of its published definition just as its paint is: a
/// published way, casing or shadow is drawn as one continuous stroke, so its
/// corners are rounded rather than mitred, and a document that states no layout
/// leaves a renderer to join them its own way. A layer type with nothing to lay
/// out (a fill, an extrusion) declares no layout block at all.
fn line_layout(role: LayerRole) -> Option<serde_json::Value> {
    match role {
        LayerRole::Way | LayerRole::Casing | LayerRole::Shadow => {
            Some(serde_json::json!({ "line-join": LINE_JOIN }))
        }
        _ => None,
    }
}

/// How the segments of a published line layer are joined: the MapLibre
/// `line-join` value that rounds a corner.
const LINE_JOIN: &str = "round";

/// How wide a label's halo is drawn, and how large its text is set, in style
/// pixels.
const LABEL_HALO_WIDTH: f64 = 1.0;
const LABEL_TEXT_SIZE: u32 = 12;

/// The features a layer selects, as a published style states a filter: an
/// `all`-rooted list of CLAUSES, each a complete expression, so a renderer that
/// evaluates them draws exactly the features the layer is for.
///
/// Three kinds of clause, in this order:
///
/// * the attribute values this layer draws, as a `match` over the attribute
///   with its value list and its true/false arms — the form that states a set
///   of values rather than a single comparison;
/// * the geometry this layer's type can render, as an `==` against
///   `geometry-type`, so a layer never tries to stroke a polygon or fill a
///   line;
/// * the narrowing a request knob asks for — the political view whose
///   alternative boundary alignment a disputed layer draws, and the categories
///   a point-of-interest band was asked for.
///
/// The ATTRIBUTES named are the ones the tiles behind these layers carry (see
/// [`vector_tile_bytes`]), so the filter a client evaluates and the features it
/// evaluates them against cannot drift apart.
fn geometry_filter(spec: &StyleLayerSpec, options: &StyleOptions) -> serde_json::Value {
    let (role, geometry) = match spec.role {
        LayerRole::Area | LayerRole::Shading | LayerRole::Extrusion => (AREA_ROLE, "Polygon"),
        LayerRole::Way | LayerRole::Casing | LayerRole::Shadow => (WAY_ROLE, "LineString"),
        LayerRole::Label | LayerRole::Icon => (POINT_ROLE, "Point"),
    };
    let mut clauses: Vec<serde_json::Value> = vec![
        serde_json::json!("all"),
        serde_json::json!(["match", ["get", ROLE_ATTRIBUTE], [role], true, false]),
        serde_json::json!(["==", ["geometry-type"], geometry]),
    ];
    // A disputed alignment is a boundary under ONE country's view, so the layer
    // that draws it selects the territories the view IN EFFECT claims, on the
    // `political_view` attribute such a feature carries — and selects nothing
    // when the request named no view, which is what leaves the de-facto
    // boundary standing.
    if style_layers::is_disputed(spec) {
        let views: Vec<&str> = options.political_view.as_deref().into_iter().collect();
        clauses.push(serde_json::json!([
            "in",
            ["get", POLITICAL_VIEW_ATTRIBUTE],
            ["literal", views],
        ]));
    } else if spec.gate == LayerGate::Poi && !options.poi_categories.is_empty() {
        // Only the requested categories are rendered, filtered on the
        // `category` attribute a point feature carries.
        clauses.push(serde_json::json!([
            "in",
            ["get", "category"],
            ["literal", options.poi_categories.clone()],
        ]));
    }
    serde_json::Value::Array(clauses)
}

/// The palette entry a style layer is painted from, and how its role shades it.
///
/// WHICH entry is decided by the TILE LAYER the style layer reads — the data
/// behind it is what gives it its colour, so every layer of one family is
/// painted from one entry. The role then shades it: a casing is the same colour
/// darkened, which is what makes it an outline, and a shadow is darker still
/// and translucent, which is what lifts a bridge off what it crosses.
fn layer_color(palette: &Palette, spec: &StyleLayerSpec) -> Hsla {
    // A masking layer hides what is drawn under it, so it is painted in the
    // ground's own colour rather than in the colour of the data it reads.
    if style_layers::is_mask(spec) {
        return palette.land;
    }
    let base = match spec.source_layer {
        WATER_LAYER => palette.water,
        ROAD_LAYER | TRAFFIC_FLOW_LAYER => palette.road,
        BUILDING_LAYER => palette.building,
        POI_LAYER | TRAFFIC_INCIDENT_LAYER => palette.marker,
        ADMIN_LAYER => palette.label,
        // Land use and the derived thin families (contours, hillshading) are
        // drawn in the palette's detail colour.
        _ => palette.graticule,
    };
    match spec.role {
        LayerRole::Casing => base.darker(CASING_DARKEN),
        LayerRole::Shadow => base.darker(2 * CASING_DARKEN).with_alpha(SHADOW_ALPHA),
        LayerRole::Shading => base.with_alpha(SHADING_ALPHA),
        _ => base,
    }
}

/// How many percentage points darker than the way it outlines a casing is
/// painted, how opaque a bridge shadow is, and how opaque shading drawn over
/// the ground is — in [`Hsla`]'s thousandths.
const CASING_DARKEN: u8 = 8;
const SHADOW_ALPHA: u16 = 300;
const SHADING_ALPHA: u16 = 400;

/// How wide a line layer draws, as the zoom interpolation a published style
/// states it with: the catalog's width where the layer comes in, growing to
/// [`WIDTH_ZOOM_FACTOR`] times that at the highest zoom the tiles reach — and
/// an interpolation's stops must ASCEND, so a layer that comes in at that zoom
/// grows over the one level above it.
///
/// Contour lines are the one family whose width a request knob sets: the
/// density asked for is how densely the available lines are rendered.
fn line_width_value(spec: &StyleLayerSpec, options: &StyleOptions) -> serde_json::Value {
    let width = if is_feature_layer(CONTOUR_FEATURE, spec.source_layer) {
        match options.contour_density.as_deref() {
            Some("High") => 1.5,
            Some("Medium") => 1.0,
            _ => 0.5,
        }
    } else {
        style_layers::line_width(spec)
    };
    let top_zoom = VECTOR_MAX_ZOOM.max(spec.min_zoom + 1);
    serde_json::json!([
        "interpolate",
        ["linear"],
        ["zoom"],
        spec.min_zoom,
        width,
        top_zoom,
        width * WIDTH_ZOOM_FACTOR,
    ])
}

/// How much wider a line is drawn at the top of the zoom range than at the
/// zoom it comes in at.
const WIDTH_ZOOM_FACTOR: f64 = 3.0;

/// How far apart point-of-interest labels are placed, in style pixels, for one
/// `PoiDensity`: a denser value places more of them, so they sit closer.
fn poi_spacing(density: Option<&str>) -> u32 {
    match density {
        Some("VerySparse") => 500,
        Some("Sparse") => 350,
        Some("Dense") => 150,
        Some("VeryDense") => 80,
        _ => 250,
    }
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

    /// The ground this tile covers, which is what decides which of the world's
    /// features it carries.
    pub fn envelope(self) -> Option<Envelope> {
        Envelope::of(&[
            self.pixel_position(0, 0),
            self.pixel_position(TILE_SIZE, TILE_SIZE),
        ])
    }
}

/// The disputed territory this tile's ground lies over, or `None` when it lies
/// over none.
///
/// The territories are [`boundaries::DISPUTED_TERRITORIES`] — the one list
/// `PoliticalView`'s own documentation gives — so a view the model adds is
/// carried into the tiles without this function being edited.
fn disputed_territory_at(coord: TileCoord) -> Option<&'static boundaries::DisputedTerritory> {
    let envelope = coord.envelope()?;
    boundaries::DISPUTED_TERRITORIES
        .iter()
        .find(|territory| territory.overlaps(envelope))
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
            let mut tags = vec![
                ("layer", TagValue::Str(name.clone())),
                ("tileset", TagValue::Str(tileset.id.clone())),
                (ROLE_ATTRIBUTE, TagValue::Str(role.to_string())),
                ("z", TagValue::Int(i64::from(coord.z))),
                ("x", TagValue::Int(i64::from(coord.x))),
                ("y", TagValue::Int(i64::from(coord.y))),
                (
                    "name",
                    TagValue::Str(format!("{name} {}/{}/{}", coord.z, coord.x, coord.y)),
                ),
                ("height", TagValue::Int(i64::from(12 + coord.z))),
            ];
            // A boundary feature over a territory whose alignment depends on
            // the political view says which view claims it, so a style's
            // disputed-boundary layer has the attribute it filters on. The
            // territories are the ONE list `PoliticalView` documents (see
            // `boundaries::DISPUTED_TERRITORIES`), matched against this tile's
            // own ground, so a tile outside every disputed territory carries no
            // disputed boundary at all.
            if name == ADMIN_LAYER {
                if let Some(territory) = disputed_territory_at(coord) {
                    tags.push((
                        POLITICAL_VIEW_ATTRIBUTE,
                        TagValue::Str(territory.view.to_string()),
                    ));
                    tags.push((
                        "territory",
                        TagValue::Str(territory.territory.to_string()),
                    ));
                }
            }
            tags
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
            &attributes(AREA_ROLE),
        );

        // A line across the tile.
        let mut line = GeometryWriter::new();
        line.move_to(margin, extent / 2);
        line.line_to(&[(extent / 2, extent / 2 - margin), (extent - margin, extent / 2)]);
        builder.add_feature(
            feature_id(coord, index, 1),
            GeomType::LineString,
            line.finish(),
            &attributes(WAY_ROLE),
        );

        // A point at the tile's centre.
        let mut point = GeometryWriter::new();
        point.move_to(extent / 2, extent / 2);
        let mut point_attributes = attributes(POINT_ROLE);
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
    let mut canvas = Canvas::new(TILE_SIZE, TILE_SIZE, tileset.palette.land.rgba());
    for row in 0..TILE_SIZE {
        for column in 0..TILE_SIZE {
            let (lon, lat) = coord.pixel_position(column, row);
            let color = terrain_color(&tileset.palette, elevation_at(lon, lat));
            canvas.put(i64::from(column), i64::from(row), color);
        }
    }
    // A one-pixel edge, which is what makes a tile seam visible at all.
    let last = i64::from(TILE_SIZE) - 1;
    canvas.hline(0, 0, last, tileset.palette.graticule.rgba());
    canvas.hline(last, 0, last, tileset.palette.graticule.rgba());
    canvas.vline(0, 0, last, tileset.palette.graticule.rgba());
    canvas.vline(last, 0, last, tileset.palette.graticule.rgba());
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

/// Lay a sheet's icons out, at `pixel_ratio` device pixels per style pixel.
///
/// The icons of a published sheet are not one size, so they are packed in rows
/// rather than in a grid of equal cells: each icon takes the next free
/// horizontal run of its own width, an icon that would cross the sheet's width
/// bound starts the next row, and a row is as tall as its tallest icon. Rows
/// are top-aligned and icons within a row abut, which is what the manifest the
/// asset endpoint serves reports (consecutive entries share a `y` and the next
/// entry's `x` is the previous one's `x` plus its width).
///
/// Every dimension scales with `pixel_ratio`, including the width bound, so the
/// `@2x` sheet is the same layout at twice the size: an icon's rectangle there
/// is exactly twice its rectangle here.
///
/// The layout is the single source for both the PNG and the JSON index, which
/// is why the index always describes the sheet it is paired with.
pub fn layout_sprites(sheet: &SpriteSheetRecord, pixel_ratio: u32) -> SpriteLayout {
    let ratio = pixel_ratio.max(1);
    let widest = sheet.icons.iter().map(|icon| icon.width).max().unwrap_or(0);
    // A sheet is never narrower than its widest icon, whatever bound the record
    // carries: an icon that cannot fit any row would never be placed.
    let bound = sheet.max_width.max(widest) * ratio;
    let mut slots = Vec::with_capacity(sheet.icons.len());
    let mut x = 0;
    let mut y = 0;
    let mut row_height = 0;
    let mut width = 0;
    for icon in &sheet.icons {
        let icon_width = (icon.width * ratio).max(1);
        let icon_height = (icon.height * ratio).max(1);
        if x > 0 && x + icon_width > bound {
            y += row_height;
            x = 0;
            row_height = 0;
        }
        slots.push(SpriteSlot {
            name: icon.name.clone(),
            x,
            y,
            width: icon_width,
            height: icon_height,
            tint: icon.tint,
            shape: icon.shape,
        });
        x += icon_width;
        row_height = row_height.max(icon_height);
        width = width.max(x);
    }
    SpriteLayout {
        slots,
        // A sheet with no icons is still a 1x1 image rather than a zero-sized
        // one, which no PNG decoder accepts.
        width: width.max(1),
        height: (y + row_height).max(1),
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
                // The inscribed disc of the icon's box, so a box that is not
                // square stays inside it.
                canvas.disc(
                    x + w / 2,
                    y + h / 2,
                    slot.width.min(slot.height) / 2,
                    slot.tint,
                );
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
/// icon, naming the rectangle it occupies in the paired PNG and the pixel ratio
/// that rectangle is measured in.
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
            }),
        );
    }
    to_json_bytes(&serde_json::Value::Object(index))
}

/// Which of a sheet's two paired files a `FileName` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpriteFileKind {
    /// The PNG sheet itself.
    Sheet,
    /// The JSON index of the sheet.
    Index,
}

/// A sprite file the model's `FileName` admits: which of the sheet's two paired
/// files it is, and the pixel ratio it is served at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpriteFile {
    pub kind: SpriteFileKind,
    pub pixel_ratio: u32,
}

/// The file kind and `pixelRatio` of a sprite file, from its name.
///
/// Matched against the exact set the modeled
/// `GetSpritesRequest$FileName` pattern (`^sprites(@2x)?\.(png|json)$`) admits,
/// rather than by looking at the extension alone, so a name the model refuses
/// is refused here too. The MEDIA TYPE is not decided here: a sprite file is
/// served as a stored object, so its `Content-Type` comes from the catalog's
/// asset-delivery record.
pub fn sprite_file(file_name: &str) -> Option<SpriteFile> {
    let (kind, pixel_ratio) = match file_name {
        "sprites.png" => (SpriteFileKind::Sheet, 1),
        "sprites@2x.png" => (SpriteFileKind::Sheet, 2),
        "sprites.json" => (SpriteFileKind::Index, 1),
        "sprites@2x.json" => (SpriteFileKind::Index, 2),
        _ => return None,
    };
    Some(SpriteFile { kind, pixel_ratio })
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
    /// The language the image's labels are written in: the request's
    /// `Language` already resolved against the catalog's label languages, so
    /// the renderer is handed a language the catalog has data for.
    pub label_language: String,
    /// The metrics the labels are measured with: the stored record for the
    /// style's own font stack, the same one `GetGlyphs` serves that stack's
    /// glyph ranges from.
    pub label_font: FontRecord,
    /// `CropLabels`: whether a label that does not fit entirely inside the
    /// frame is left off the image rather than drawn and cut by its edge.
    pub crop_labels: bool,
    /// `PoliticalView`: the country code whose view of a disputed boundary the
    /// image draws, or `None` for the de-facto boundaries.
    pub political_view: Option<String>,
}

impl StaticMapView {
    /// The image size in device pixels.
    pub fn pixel_size(&self) -> (u32, u32) {
        (self.width * self.pixel_ratio, self.height * self.pixel_ratio)
    }
}

/// Render a static map: the terrain field inside the framed envelope, a
/// graticule, the political boundaries the requested view recognises, the
/// requested markers and overlay, the places the frame carries with their
/// labels, and a scale bar.
pub fn render_static_map(palette: &Palette, view: &StaticMapView) -> Vec<u8> {
    let (pixel_width, pixel_height) = view.pixel_size();
    let mut canvas = Canvas::new(pixel_width, pixel_height, palette.land.rgba());
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

    // The political boundaries inside the frame, at the lines the requested
    // view recognises. Drawn under the overlay and the annotations, the way a
    // basemap's `borders` layer sits under them.
    for line in boundaries::boundary_lines(view.envelope, view.political_view.as_deref()) {
        for pair in line.windows(2) {
            thick_line(
                &mut canvas,
                project(pair[0]),
                project(pair[1]),
                palette.label.rgba(),
                view.pixel_ratio,
            );
        }
    }

    for shape in &view.overlay {
        draw_overlay_shape(&mut canvas, palette, shape, &project);
    }

    if view.show_pois {
        // The places the frame carries: the icon and its label are one layer of
        // a map (the style document's `places` symbol layer draws both), so one
        // switch shows or hides both.
        let label_size = labels::LABEL_TEXT_SIZE * view.label_scale * view.pixel_ratio;
        let icon_radius = 2 * view.pixel_ratio;
        for (position, name) in labelled_places(view.envelope, &view.label_language) {
            let (px, py) = project(position);
            canvas.disc(px, py, icon_radius, palette.label.rgba());
            // The label sits to the right of its icon, on the icon's own
            // baseline, the way a symbol layer places text beside an icon.
            let anchor = (
                px + i64::from(icon_radius) + i64::from(2 * view.pixel_ratio),
                py + i64::from(label_size) / 3,
            );
            let (label_width, label_height) =
                labels::label_extent(&view.label_font, &name, label_size);
            // `CropLabels` "prevents the labels that are on the edge of the
            // image from being cut or obscured": a label that does not fit
            // inside the frame is left off the image entirely rather than drawn
            // across its edge.
            if view.crop_labels
                && !fits_inside(
                    anchor,
                    (label_width, label_height),
                    inset,
                    (frame_width, frame_height),
                )
            {
                continue;
            }
            labels::draw_label(
                &mut canvas,
                &view.label_font,
                &name,
                label_size,
                anchor,
                palette.label.rgba(),
            );
        }
    }

    for marker in &view.markers {
        let (px, py) = project(*marker);
        canvas.disc(
            px,
            py,
            3 * view.label_scale * view.pixel_ratio,
            palette.marker.rgba(),
        );
    }

    if let Some(center) = view.center {
        let (px, py) = project(center);
        let radius = 5 * view.label_scale * view.pixel_ratio;
        canvas.disc(px, py, radius, palette.marker.rgba());
        canvas.hline(
            py,
            px.saturating_sub(i64::from(radius)),
            px.saturating_add(i64::from(radius)),
            palette.label.rgba(),
        );
        canvas.vline(
            px,
            py.saturating_sub(i64::from(radius)),
            py.saturating_add(i64::from(radius)),
            palette.label.rgba(),
        );
    }

    draw_scale_bar(&mut canvas, palette, view, frame_width);

    // The frame edge, so the padded border is visibly a border.
    let right = i64::from(pixel_width) - 1 - i64::from(inset);
    let bottom = i64::from(pixel_height) - 1 - i64::from(inset);
    canvas.hline(
        i64::from(inset),
        i64::from(inset),
        right,
        palette.graticule.rgba(),
    );
    canvas.hline(bottom, i64::from(inset), right, palette.graticule.rgba());
    canvas.vline(
        i64::from(inset),
        i64::from(inset),
        bottom,
        palette.graticule.rgba(),
    );
    canvas.vline(right, i64::from(inset), bottom, palette.graticule.rgba());

    canvas.to_jpeg()
}

/// How many place candidates there are per axis of the frame.
const PLACE_LATTICE: u32 = 5;

/// The places an image framed by `envelope` carries, each with the name it is
/// labelled with in `language`.
///
/// A fixed lattice over the frame, kept where the terrain field says there is
/// land, so a place sits on ground rather than in the sea — the same field
/// every other asset of this service samples. One function, so the dots drawn,
/// the labels drawn beside them and what a caller expects a frame to carry are
/// all the same list.
pub fn labelled_places(envelope: Envelope, language: &str) -> Vec<(Position, String)> {
    let mut places = Vec::new();
    for ix in 0..PLACE_LATTICE {
        for iy in 0..PLACE_LATTICE {
            let fx = (f64::from(ix) + 0.5) / f64::from(PLACE_LATTICE);
            let fy = (f64::from(iy) + 0.5) / f64::from(PLACE_LATTICE);
            let position = envelope.unproject_fraction(fx, fy);
            if elevation_at(position.0, position.1) < 0.0 {
                continue;
            }
            places.push((position, labels::place_name(position, language)));
        }
    }
    places
}

/// Whether a label whose pen starts at `anchor` and whose extent is
/// `(width, height)` lies entirely inside the frame.
fn fits_inside(anchor: (i64, i64), extent: (u32, u32), inset: u32, frame: (u32, u32)) -> bool {
    let (frame_width, frame_height) = frame;
    let left = i64::from(inset);
    let top = left;
    let right = left + i64::from(frame_width) - 1;
    let bottom = top + i64::from(frame_height) - 1;
    let label_right = anchor.0 + i64::from(extent.0) - 1;
    let label_top = anchor.1 - i64::from(extent.1) + 1;
    anchor.0 >= left && label_right <= right && label_top >= top && anchor.1 <= bottom
}

/// Draw a line `thickness` pixels wide, so a one-pixel feature stays visible at
/// a higher device pixel ratio.
fn thick_line(canvas: &mut Canvas, from: (i64, i64), to: (i64, i64), color: Rgba, thickness: u32) {
    for offset in 0..i64::from(thickness.max(1)) {
        canvas.line((from.0, from.1 + offset), (to.0, to.1 + offset), color);
        canvas.line((from.0 + offset, from.1), (to.0 + offset, to.1), color);
    }
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
            palette.graticule.rgba(),
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
            palette.graticule.rgba(),
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
            canvas.disc(px, py, 4, palette.marker.rgba());
        }
        OverlayShape::Line(points) => {
            for pair in points.windows(2) {
                canvas.line(project(pair[0]), project(pair[1]), palette.marker.rgba());
            }
        }
        OverlayShape::Polygon(points) => {
            for pair in points.windows(2) {
                canvas.line(project(pair[0]), project(pair[1]), palette.marker.rgba());
            }
            if let (Some(first), Some(last)) = (points.first(), points.last()) {
                canvas.line(project(*last), project(*first), palette.marker.rgba());
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
    canvas.fill_rect(x, y, length, thickness, palette.label.rgba());
    let tick = i64::from(4 * view.label_scale * view.pixel_ratio);
    canvas.vline(
        x,
        y - tick,
        y + i64::from(thickness),
        palette.label.rgba(),
    );
    canvas.vline(
        x + i64::from(length),
        y - tick,
        y + i64::from(thickness),
        palette.label.rgba(),
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
