// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-geomaps-mapsservice service: the map asset catalog.
//!
//! Geo Maps is a read-only content API — every operation it binds is
//! `@readonly` and there is no CRUD lifecycle — so this service's state is not
//! a resource table but an **asset catalog**: the style records, tilesets,
//! sprite sheets, font stacks, content types, cache directives and pricing
//! buckets that decide what each handler serves.
//!
//! The catalog is built by [`Default`], which is what `engine.state::<T>()`
//! calls the first time any handler touches this service's store. Seeding there
//! rather than from a handler is what keeps every operation a true read: no
//! request ever writes to this state, so what one caller sees is exactly what
//! the next one sees.
//!
//! Asset BYTES are not kept pre-rendered. Each handler derives them from the
//! catalog record plus the request's own addressing — tile coordinates, sprite
//! scale, glyph range, static-map framing — through [`assets`]. The derivation
//! is a pure function of (record, address), which gives the two properties a map
//! client needs: the same request always answers with byte-identical content and
//! therefore the same `ETag`, and changing a catalog record changes every asset
//! derived from it.

pub mod assets;
pub mod canvas;
pub mod glyph_pbf;
pub mod vector_tile;

use std::collections::BTreeMap;

/// An 8-bit RGBA colour: the one colour representation the catalog, the
/// renderer and the style documents share.
pub type Rgba = [u8; 4];

/// The model's `MapStyle` enum (`Standard | Monochrome | Hybrid | Satellite`),
/// as a catalog key. The wire values are the ones `ops::types::MAP_STYLE_VALUES`
/// lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MapStyleKind {
    Standard,
    Monochrome,
    Hybrid,
    Satellite,
}

impl MapStyleKind {
    /// Every value the model declares, in catalog order.
    pub const ALL: [Self; 4] = [
        Self::Standard,
        Self::Monochrome,
        Self::Hybrid,
        Self::Satellite,
    ];

    /// The wire value, or `None` when the string names no modeled style.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "Standard" => Some(Self::Standard),
            "Monochrome" => Some(Self::Monochrome),
            "Hybrid" => Some(Self::Hybrid),
            "Satellite" => Some(Self::Satellite),
            _ => None,
        }
    }

    /// This style as it appears on the wire and in a URL path.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Monochrome => "Monochrome",
            Self::Hybrid => "Hybrid",
            Self::Satellite => "Satellite",
        }
    }
}

/// The model's `ColorScheme` enum (`Light | Dark`), as a catalog key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ColorSchemeKind {
    Light,
    Dark,
}

impl ColorSchemeKind {
    /// Every value the model declares.
    pub const ALL: [Self; 2] = [Self::Light, Self::Dark];

    /// The wire value, or `None` when the string names no modeled scheme.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "Light" => Some(Self::Light),
            "Dark" => Some(Self::Dark),
            _ => None,
        }
    }

    /// This scheme as it appears on the wire and in a URL path.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

/// The model's `Variant` enum, which declares exactly one value today. It is
/// still a catalog key, because `GetSprites` binds it as its own path label and
/// a second variant would be a second sheet rather than a different style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VariantKind {
    Default,
}

impl VariantKind {
    /// Every value the model declares.
    pub const ALL: [Self; 1] = [Self::Default];

    /// The wire value, or `None` when the string names no modeled variant.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "Default" => Some(Self::Default),
            _ => None,
        }
    }

    /// This variant as it appears on the wire and in a URL path.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Default => "Default",
        }
    }
}

/// The colours one (style, colour scheme) pair paints with. Every raster asset
/// and every style-document paint property reads them from here, so a style's
/// look is catalog data rather than a constant in a handler.
#[derive(Debug, Clone)]
pub struct Palette {
    pub land: Rgba,
    pub water: Rgba,
    pub road: Rgba,
    pub building: Rgba,
    pub graticule: Rgba,
    pub label: Rgba,
    pub marker: Rgba,
}

/// One source a style document declares: the tileset it reads and the URL
/// template a client follows to `GetTile` for it.
#[derive(Debug, Clone)]
pub struct StyleSource {
    /// The source name inside the style document (`basemap`, `satellite`, ...).
    pub name: String,
    /// The `Tileset` path label `GetTile` takes.
    pub tileset: String,
    /// The `{z}/{x}/{y}` template a client expands to reach `GetTile`.
    pub tile_url_template: String,
}

/// The stored half of a style descriptor: everything that does not depend on
/// the request's feature knobs.
#[derive(Debug, Clone)]
pub struct StyleRecord {
    pub style: MapStyleKind,
    pub color_scheme: ColorSchemeKind,
    /// Human name the style document reports.
    pub name: String,
    pub palette: Palette,
    /// The always-on sources, in draw order.
    pub sources: Vec<StyleSource>,
    /// The `GetSprites` URL template, without the `.png`/`.json` suffix a
    /// client appends (the MapLibre sprite convention).
    pub sprite_url_template: String,
    /// The `GetGlyphs` URL template, with the `{fontstack}`/`{range}`
    /// placeholders a client expands.
    pub glyph_url_template: String,
    /// The font stack the style's symbol layers ask for.
    pub font_stack: Vec<String>,
    pub content_type: String,
    pub cache_control: String,
}

/// How a tileset's tiles are encoded. It decides both the bytes a tile carries
/// and the `Content-Type` reported for it, so the two can never disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileFormat {
    /// A Mapbox Vector Tile (protobuf), built in [`vector_tile`].
    VectorMvt,
    /// A baseline JPEG raster tile, the encoding photographic imagery is
    /// served in.
    RasterJpeg,
    /// An RGB PNG raster tile whose pixels carry terrain elevation in the
    /// Terrarium encoding (`elevation = (r * 256 + g + b / 256) - 32768`).
    /// Lossless, because a lossy pixel is a wrong elevation.
    RasterDemPng,
}

/// A tileset `GetTile` serves: its addressing rules, its encoding and the
/// pricing bucket requests against it are charged to.
#[derive(Debug, Clone)]
pub struct TilesetRecord {
    pub id: String,
    pub format: TileFormat,
    pub content_type: String,
    /// Lowest zoom this tileset has tiles for; a lower `Z` is out of range.
    pub min_zoom: u32,
    /// Highest zoom this tileset has tiles for; a higher `Z` is out of range.
    pub max_zoom: u32,
    /// The layer names a vector tile from this tileset carries. Empty for a
    /// raster tileset.
    pub layers: Vec<String>,
    /// Colours a raster tile from this tileset is drawn with.
    pub palette: Palette,
    /// The bucket `GetTile` reports; `None` falls back to
    /// [`ComAmazonawsGeomapsMapsserviceState::default_pricing_bucket`].
    pub pricing_bucket: Option<String>,
    pub cache_control: String,
}

/// The shape a sprite icon is drawn as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconShape {
    Disc,
    Square,
    Pin,
}

/// One icon in a sprite sheet: the name a style layer references, its size at
/// `pixelRatio` 1, its shape and its colour.
#[derive(Debug, Clone)]
pub struct IconSpec {
    pub name: String,
    pub size: u32,
    pub shape: IconShape,
    pub tint: Rgba,
}

/// A sprite sheet: the icons it holds and how they are laid out on it.
#[derive(Debug, Clone)]
pub struct SpriteSheetRecord {
    pub style: MapStyleKind,
    pub color_scheme: ColorSchemeKind,
    pub variant: VariantKind,
    pub icons: Vec<IconSpec>,
    /// Icons per row on the sheet.
    pub columns: u32,
    /// Transparent gap kept around each icon, at `pixelRatio` 1.
    pub gutter: u32,
    pub cache_control: String,
}

/// A font stack `GetGlyphs` serves, and the metrics its glyph range is built
/// from.
#[derive(Debug, Clone)]
pub struct FontRecord {
    pub name: String,
    /// SDF em size in pixels (the size MapLibre rasterizes glyph ranges at).
    pub em: u32,
    /// Pixels of SDF padding kept on each side of a glyph bitmap.
    pub buffer: u32,
    /// Horizontal advance for an em-wide glyph, in pixels.
    pub advance: u32,
    /// Ascender height in pixels, measured from the baseline.
    pub ascender: i32,
    pub content_type: String,
    pub cache_control: String,
}

/// What `GetStaticMap` reports around the image it renders.
#[derive(Debug, Clone)]
pub struct StaticMapRecord {
    pub content_type: String,
    pub cache_control: String,
    /// The bucket `GetStaticMap` reports; `None` falls back to
    /// [`ComAmazonawsGeomapsMapsserviceState::default_pricing_bucket`].
    pub pricing_bucket: Option<String>,
}

/// Per-service state for com-amazonaws-geomaps-mapsservice. A handler reaches it through [`service_state`].
///
/// Seeded by [`Default`] (see the module docs): a handler only ever reads it.
#[derive(Debug)]
pub struct ComAmazonawsGeomapsMapsserviceState {
    /// Style records, keyed the way `GetStyleDescriptor` addresses them.
    pub styles: BTreeMap<(MapStyleKind, ColorSchemeKind), StyleRecord>,
    /// Tilesets, keyed by the `Tileset` path label `GetTile` takes. A label
    /// that is not a key here is a `ResourceNotFoundException`.
    pub tilesets: BTreeMap<String, TilesetRecord>,
    /// Sprite sheets, keyed the way `GetSprites` addresses them.
    pub sprites: BTreeMap<(MapStyleKind, ColorSchemeKind, VariantKind), SpriteSheetRecord>,
    /// Named font stacks `GetGlyphs` has metrics for.
    pub fonts: BTreeMap<String, FontRecord>,
    /// Metrics used for a font stack that is not in [`Self::fonts`].
    /// `GetGlyphs` models no error, so every modeled-valid stack must answer.
    pub fallback_font: FontRecord,
    pub static_maps: StaticMapRecord,
    /// The pricing bucket reported for an asset whose own record carries none.
    pub default_pricing_bucket: String,
}

impl Default for ComAmazonawsGeomapsMapsserviceState {
    fn default() -> Self {
        seed()
    }
}

impl ComAmazonawsGeomapsMapsserviceState {
    /// The pricing bucket to report for an asset record, falling back to
    /// [`Self::default_pricing_bucket`]. `PricingBucket` is `@required` on both
    /// `GetTile` and `GetStaticMap`, so this never answers an empty string.
    pub fn pricing_bucket(&self, record: Option<&String>) -> String {
        record
            .cloned()
            .unwrap_or_else(|| self.default_pricing_bucket.clone())
    }
}

/// Build the seeded catalog.
///
/// Deterministic and allocation-only: no clock, no randomness, no rendering, so
/// two emulators seed identical catalogs and the first request pays almost
/// nothing for it.
fn seed() -> ComAmazonawsGeomapsMapsserviceState {
    let mut tilesets = BTreeMap::new();
    for record in seed_tilesets() {
        tilesets.insert(record.id.clone(), record);
    }

    let mut styles = BTreeMap::new();
    let mut sprites = BTreeMap::new();
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            styles.insert((style, scheme), seed_style(style, scheme));
            for variant in VariantKind::ALL {
                sprites.insert(
                    (style, scheme, variant),
                    seed_sprite_sheet(style, scheme, variant),
                );
            }
        }
    }

    let mut fonts = BTreeMap::new();
    for record in seed_fonts() {
        fonts.insert(record.name.clone(), record);
    }

    ComAmazonawsGeomapsMapsserviceState {
        styles,
        tilesets,
        sprites,
        fonts,
        fallback_font: font_record("Standard Regular", 24, 18, 20),
        static_maps: StaticMapRecord {
            // `GetStaticMapResponse$Blob` documents the body as a jpeg.
            content_type: "image/jpeg".to_string(),
            cache_control: "max-age=300".to_string(),
            pricing_bucket: Some("StaticMaps".to_string()),
        },
        default_pricing_bucket: "MapAssets".to_string(),
    }
}

/// The palette for one (style, colour scheme) pair.
fn palette(style: MapStyleKind, scheme: ColorSchemeKind) -> Palette {
    match (style, scheme) {
        (MapStyleKind::Standard, ColorSchemeKind::Light) => Palette {
            land: [0xf7, 0xf5, 0xf2, 0xff],
            water: [0xa0, 0xc8, 0xf0, 0xff],
            road: [0xff, 0xff, 0xff, 0xff],
            building: [0xe4, 0xdf, 0xd6, 0xff],
            graticule: [0xd9, 0xd2, 0xc7, 0xff],
            label: [0x33, 0x33, 0x33, 0xff],
            marker: [0xd9, 0x42, 0x0b, 0xff],
        },
        (MapStyleKind::Standard, ColorSchemeKind::Dark) => Palette {
            land: [0x26, 0x28, 0x2b, 0xff],
            water: [0x15, 0x29, 0x3f, 0xff],
            road: [0x3b, 0x3f, 0x44, 0xff],
            building: [0x31, 0x34, 0x38, 0xff],
            graticule: [0x3a, 0x3d, 0x41, 0xff],
            label: [0xe6, 0xe6, 0xe6, 0xff],
            marker: [0xff, 0x70, 0x43, 0xff],
        },
        (MapStyleKind::Monochrome, ColorSchemeKind::Light) => Palette {
            land: [0xf2, 0xf2, 0xf2, 0xff],
            water: [0xd9, 0xd9, 0xd9, 0xff],
            road: [0xff, 0xff, 0xff, 0xff],
            building: [0xe6, 0xe6, 0xe6, 0xff],
            graticule: [0xcc, 0xcc, 0xcc, 0xff],
            label: [0x26, 0x26, 0x26, 0xff],
            marker: [0x59, 0x59, 0x59, 0xff],
        },
        (MapStyleKind::Monochrome, ColorSchemeKind::Dark) => Palette {
            land: [0x1f, 0x1f, 0x1f, 0xff],
            water: [0x2e, 0x2e, 0x2e, 0xff],
            road: [0x4d, 0x4d, 0x4d, 0xff],
            building: [0x33, 0x33, 0x33, 0xff],
            graticule: [0x3d, 0x3d, 0x3d, 0xff],
            label: [0xf2, 0xf2, 0xf2, 0xff],
            marker: [0xa6, 0xa6, 0xa6, 0xff],
        },
        (MapStyleKind::Hybrid | MapStyleKind::Satellite, ColorSchemeKind::Light) => Palette {
            land: [0x4a, 0x5d, 0x3a, 0xff],
            water: [0x1b, 0x3a, 0x5c, 0xff],
            road: [0xe8, 0xe0, 0xc0, 0xff],
            building: [0x6b, 0x66, 0x5c, 0xff],
            graticule: [0x7d, 0x8a, 0x6f, 0xff],
            label: [0xff, 0xff, 0xff, 0xff],
            marker: [0xff, 0xd5, 0x4f, 0xff],
        },
        (MapStyleKind::Hybrid | MapStyleKind::Satellite, ColorSchemeKind::Dark) => Palette {
            land: [0x2b, 0x36, 0x22, 0xff],
            water: [0x0f, 0x21, 0x34, 0xff],
            road: [0xb8, 0xb0, 0x92, 0xff],
            building: [0x45, 0x42, 0x3b, 0xff],
            graticule: [0x4a, 0x53, 0x42, 0xff],
            label: [0xf0, 0xf0, 0xf0, 0xff],
            marker: [0xff, 0xc1, 0x07, 0xff],
        },
    }
}

/// The tilesets the documented `Tileset` values name.
///
/// The value list is `GetTileRequest$Tileset`'s own documentation:
/// `raster.satellite | vector.basemap | vector.traffic | raster.dem`
/// (`smithy_query(query="doc", shape="GetTileRequest", member="Tileset")`).
fn seed_tilesets() -> Vec<TilesetRecord> {
    let basemap_palette = palette(MapStyleKind::Standard, ColorSchemeKind::Light);
    let satellite_palette = palette(MapStyleKind::Satellite, ColorSchemeKind::Light);
    vec![
        TilesetRecord {
            id: "vector.basemap".to_string(),
            format: TileFormat::VectorMvt,
            content_type: "application/vnd.mapbox-vector-tile".to_string(),
            min_zoom: 0,
            max_zoom: 16,
            layers: vec![
                "land".to_string(),
                "water".to_string(),
                "roads".to_string(),
                "buildings".to_string(),
                "places".to_string(),
                "borders".to_string(),
            ],
            palette: basemap_palette.clone(),
            pricing_bucket: Some("MapTiles".to_string()),
            cache_control: "max-age=86400".to_string(),
        },
        TilesetRecord {
            id: "vector.traffic".to_string(),
            format: TileFormat::VectorMvt,
            content_type: "application/vnd.mapbox-vector-tile".to_string(),
            min_zoom: 0,
            max_zoom: 16,
            layers: vec![
                "traffic_flow".to_string(),
                "traffic_incidents".to_string(),
            ],
            palette: basemap_palette,
            pricing_bucket: Some("TrafficTiles".to_string()),
            cache_control: "max-age=60".to_string(),
        },
        TilesetRecord {
            id: "raster.satellite".to_string(),
            format: TileFormat::RasterJpeg,
            content_type: "image/jpeg".to_string(),
            min_zoom: 0,
            max_zoom: 19,
            layers: Vec::new(),
            palette: satellite_palette,
            pricing_bucket: Some("SatelliteTiles".to_string()),
            cache_control: "max-age=86400".to_string(),
        },
        TilesetRecord {
            id: "raster.dem".to_string(),
            format: TileFormat::RasterDemPng,
            content_type: "image/png".to_string(),
            min_zoom: 0,
            max_zoom: 14,
            layers: Vec::new(),
            // A DEM tile's pixels are elevation, not colour; the palette is
            // carried for uniformity and is unused by the Terrarium encoding.
            palette: palette(MapStyleKind::Monochrome, ColorSchemeKind::Light),
            // Deliberately none: this record exercises the
            // `default_pricing_bucket` fallback.
            pricing_bucket: None,
            cache_control: "max-age=86400".to_string(),
        },
    ]
}

/// The style record for one (style, colour scheme) pair.
fn seed_style(style: MapStyleKind, scheme: ColorSchemeKind) -> StyleRecord {
    let mut sources = Vec::new();
    if matches!(style, MapStyleKind::Hybrid | MapStyleKind::Satellite) {
        sources.push(style_source("satellite", "raster.satellite"));
    }
    if !matches!(style, MapStyleKind::Satellite) {
        sources.push(style_source("basemap", "vector.basemap"));
    }
    StyleRecord {
        style,
        color_scheme: scheme,
        name: format!("{} {}", style.as_wire(), scheme.as_wire()),
        palette: palette(style, scheme),
        sources,
        sprite_url_template: assets::sprite_url_template(style, scheme, VariantKind::Default),
        glyph_url_template: assets::GLYPH_URL_TEMPLATE.to_string(),
        font_stack: match style {
            MapStyleKind::Monochrome => vec!["Amazon Ember Condensed RC Regular".to_string()],
            _ => vec!["Amazon Ember Regular".to_string()],
        },
        content_type: "application/json".to_string(),
        cache_control: "max-age=3600".to_string(),
    }
}

fn style_source(name: &str, tileset: &str) -> StyleSource {
    StyleSource {
        name: name.to_string(),
        tileset: tileset.to_string(),
        tile_url_template: assets::tile_url_template(tileset),
    }
}

/// The sprite sheet for one (style, colour scheme, variant) triple. The icon
/// tints come from that pair's palette, so the `Dark` sheet really is a
/// different sheet rather than the same bytes under another key.
fn seed_sprite_sheet(
    style: MapStyleKind,
    scheme: ColorSchemeKind,
    variant: VariantKind,
) -> SpriteSheetRecord {
    let p = palette(style, scheme);
    let icons = vec![
        IconSpec {
            name: "poi-dot".to_string(),
            size: 12,
            shape: IconShape::Disc,
            tint: p.marker,
        },
        IconSpec {
            name: "poi-restaurant".to_string(),
            size: 16,
            shape: IconShape::Disc,
            tint: p.label,
        },
        IconSpec {
            name: "poi-hotel".to_string(),
            size: 16,
            shape: IconShape::Square,
            tint: p.building,
        },
        IconSpec {
            name: "poi-park".to_string(),
            size: 16,
            shape: IconShape::Disc,
            tint: p.graticule,
        },
        IconSpec {
            name: "transit-rail".to_string(),
            size: 16,
            shape: IconShape::Square,
            tint: p.road,
        },
        IconSpec {
            name: "marker".to_string(),
            size: 20,
            shape: IconShape::Pin,
            tint: p.marker,
        },
    ];
    SpriteSheetRecord {
        style,
        color_scheme: scheme,
        variant,
        icons,
        columns: 3,
        gutter: 2,
        cache_control: "max-age=86400".to_string(),
    }
}

/// The font stacks this catalog has metrics for.
fn seed_fonts() -> Vec<FontRecord> {
    vec![
        font_record("Amazon Ember Regular", 24, 18, 20),
        font_record("Amazon Ember Bold", 24, 20, 20),
        font_record("Amazon Ember Condensed RC Regular", 24, 15, 20),
        font_record("Noto Sans Regular", 24, 19, 21),
    ]
}

fn font_record(name: &str, em: u32, advance: u32, ascender: i32) -> FontRecord {
    FontRecord {
        name: name.to_string(),
        em,
        // 3px is the SDF buffer the MapLibre glyph pipeline uses, so a client
        // that assumes it reads our bitmaps correctly.
        buffer: 3,
        advance,
        ascender,
        content_type: "application/octet-stream".to_string(),
        cache_control: "max-age=86400".to_string(),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGeomapsMapsserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGeomapsMapsserviceState>)
}
