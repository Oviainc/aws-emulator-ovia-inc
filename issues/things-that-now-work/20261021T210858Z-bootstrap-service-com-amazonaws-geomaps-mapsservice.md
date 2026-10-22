Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Geo Maps (MapsService) emulator

_Filed: 2026-10-21T21:08:58Z_
_Operation: service-com-amazonaws-geomaps-mapsservice | Class: stub_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in this crate is an unmodified scaffold: all five files under `src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answer HTTP 501 `NotImplemented`, and `src/state.rs` holds `ComAmazonawsGeomapsMapsserviceState {}` with no fields (only a `// TODO: add service-specific state fields` comment). A map client cannot render anything against this emulator today.

Geo Maps is a read-only content-serving API (`restJson1`, sigv4 name `geo-maps`) with no CRUD lifecycle — the primary resource is a *map asset* (style descriptor, tile, sprite sheet, glyph range, rendered static map). The happy path to implement first is therefore the standard MapLibre bootstrap sequence a map viewer performs on load, in this order:

1. **GetStyleDescriptor** — `GET /v2/styles/{Style}/descriptor`. Input: `Style` (MapStyle: Hybrid|Monochrome|Satellite|Standard, httpLabel, required), plus the optional query knobs `ColorScheme` (Dark|Light), `PoliticalView`, `Terrain`, `ContourDensity`, `Traffic`, `TravelModes`, `Buildings`, `PoiDensity`, `PoiCategories`, `Key`. Output: `Blob` (httpPayload — a JSON style document), `ContentType`, `CacheControl`, `ETag`. The returned style JSON must be the one stored for that (Style, ColorScheme, PoliticalView) key in the seeded catalog, and must reference the tile/sprite/glyph URL templates the next three calls serve — not a constant blob ignoring `Style`.
2. **GetTile** — `GET /v2/tiles/{Tileset}/{Z}/{X}/{Y}`. Input: `Tileset` (len 1..100, pattern `^[-.\w]+$`, required), `Z`, `X`, `Y` (httpLabel strings, required), `AdditionalFeatures` (list, max 4), `Key`. Output: `Blob` (httpPayload), `ContentType`, `CacheControl`, `ETag`, and the **required** `PricingBucket` header (`x-amz-geo-pricing-bucket`). Must return the bytes recorded in the tile store for the exact (tileset, z, x, y) tuple; an unknown tileset must be `ResourceNotFoundException` (404) and a malformed/out-of-range z/x/y must be `ValidationException` (400) — both are modeled on this operation.
3. **GetSprites** — `GET /v2/styles/{Style}/{ColorScheme}/{Variant}/sprites/{FileName}`. Input: `Style`, `ColorScheme`, `Variant` (Default), `FileName` (all httpLabel, required). Output: `Blob`, `ContentType`, `CacheControl`, `ETag`. Must serve the sprite sheet stored for that 4-tuple, with `ContentType` derived from the `FileName` extension (`image/png` for `.png`, `application/json` for `.json`), and the `.json` index must describe the icons in the `.png` it is paired with.
4. **GetGlyphs** — `GET /v2/glyphs/{FontStack}/{FontUnicodeRange}`. Input: `FontStack`, `FontUnicodeRange` (httpLabel, required). Output: `Blob`, `ContentType`, `CacheControl`, `ETag`. Must serve the PBF glyph range stored for that (font stack, unicode range) pair.
5. **GetStaticMap** — `GET /v2/static/{FileName}`. Input: `FileName` (httpLabel, required), `Width` and `Height` (httpQuery, **required**), plus the framing/styling query set: `BoundingBox`, `BoundedPositions`, `Center`, `Zoom`, `Radius` (default 0), `Padding`, `Style` (Satellite|Standard), `ColorScheme`, `LabelSize`, `Language`, `PoliticalView`, `PointsOfInterests`, `ScaleBarUnit`, `CropLabels`, `CompactOverlay`, `GeoJsonOverlay`, `Key`. Output: `Blob` (the rendered image), `ContentType`, `CacheControl`, `ETag`, required `PricingBucket`. Must reject a request that supplies no framing at all (none of `BoundingBox`/`BoundedPositions`/`Center`), that violates the documented `Center`/`BoundingBox` patterns, or that omits `Width`/`Height`, with `ValidationException` (400); on success the returned image bytes and `ContentType` must be derived from the requested `FileName` extension, `Width`/`Height` and `Style`, and the same request must be byte-identical on repeat (same `ETag`).

NEVER STUBS: each of the five must compute its response from the persisted asset catalog in the shared engine state. A hardcoded blob, a 200 with an empty payload, an echoed input, or a randomly generated `ETag` that is not stored is a failure of this issue. `ETag` must be a stable hash of the bytes actually returned, repeat requests for the same asset must return the same `ETag`, and `PricingBucket` must be a real stored/derived value on GetTile and GetStaticMap because the model marks it `@required` (omitting it breaks serialization fidelity for every SDK client).

## State Model

Populate `ComAmazonawsGeomapsMapsserviceState` (in `src/state.rs`) with a seeded, deterministic asset catalog — seeded once at first access, then read by every handler through `crate::state::service_state(&*self.engine)`:

```
ComAmazonawsGeomapsMapsserviceState {
    // GetStyleDescriptor
    styles: Map<StyleKey, StyleDescriptor>,
    //   StyleKey { style: MapStyle, color_scheme: ColorScheme, political_view: Option<String> }
    //   StyleDescriptor { body: Vec<u8> /* style JSON */, content_type: String,
    //                     cache_control: String, etag: String,
    //                     tile_url_template: String, sprite_url_template: String,
    //                     glyph_url_template: String }

    // GetTile
    tilesets: Map<String /* tileset id */, Tileset>,
    //   Tileset { id: String, content_type: String, min_zoom: u8, max_zoom: u8,
    //             tiles: Map<(z: u32, x: u32, y: u32), TileAsset>,
    //             pricing_bucket: String }
    //   TileAsset { blob: Vec<u8>, etag: String, cache_control: String }

    // GetSprites
    sprites: Map<SpriteKey, Asset>,
    //   SpriteKey { style: MapStyle, color_scheme: ColorScheme, variant: Variant, file_name: String }

    // GetGlyphs
    glyphs: Map<(font_stack: String, unicode_range: String), Asset>,

    // GetStaticMap: render cache, keyed by the normalized request so a repeated
    // request is byte-identical and carries the same ETag
    static_maps: Map<StaticMapKey, Asset>,
    //   StaticMapKey { file_name, width, height, style, color_scheme, framing /* bbox|positions|center+zoom|radius */,
    //                  padding, label_size, language, political_view, pois, scale_unit, crop_labels, overlay_digest }

    // Shared
    //   Asset { blob: Vec<u8>, content_type: String, cache_control: String, etag: String,
    //           pricing_bucket: Option<String> }
    default_pricing_bucket: String,
}
```

Operations and parameters outside this first slice (e.g. the full `GeoJsonOverlay`/`CompactOverlay` rendering semantics, `Terrain`/`Traffic`/`PoiCategories` style-descriptor filtering fidelity, throttling and `AccessDeniedException` paths) may remain unimplemented and will be filed as individual gap issues later. Done = a client script can fetch a style descriptor, follow its templates to fetch a tile, a sprite sheet (png + json) and a glyph range, and fetch a static map, with correct `Content-Type`/`ETag`/`PricingBucket` headers, stable repeat responses, and 404/400 on the unknown-tileset and invalid-parameter paths.

## Evidence

`services/service-com-amazonaws-geomaps-mapsservice/OPERATIONS.md` lists all 5 bound operations with their routes, inputs, outputs and errors: GetGlyphs (`GET /v2/glyphs/{FontStack}/{FontUnicodeRange}`), GetSprites (`GET /v2/styles/{Style}/{ColorScheme}/{Variant}/sprites/{FileName}`), GetStaticMap (`GET /v2/static/{FileName}`, required `Width`/`Height`, errors AccessDenied/Internal/Throttling/Validation, required `PricingBucket` output header), GetStyleDescriptor (`GET /v2/styles/{Style}/descriptor`), GetTile (`GET /v2/tiles/{Tileset}/{Z}/{X}/{Y}`, errors including ResourceNotFoundException 404 and ValidationException 400, required `PricingBucket`). OPERATIONS.md also fixes the wire protocol as `restJson1` with sigv4 signing name `geo-maps`.

Stub census: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-geomaps-mapsservice/src/ops/` matches all five handler files (`get_glyphs.rs`, `get_sprites.rs`, `get_static_map.rs`, `get_style_descriptor.rs`, `get_tile.rs`), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. `src/state.rs` defines `ComAmazonawsGeomapsMapsserviceState` with zero fields, proving no state is held. No tests assert any of this behavior (only the generated `tests/state_roundtrip.rs` example).

Grounding for the workflow itself: the style-descriptor → tiles/sprites/glyphs fan-out is the documented Amazon Location Service / MapLibre map-rendering loop, and the required `PricingBucket` header plus the ResourceNotFound/Validation error set are declared in the model as rendered in OPERATIONS.md.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `geo-maps` client (or signed curl) at it.
2. GET /v2/styles/Standard/descriptor?color-scheme=Light — expect 200 with a JSON style document payload, `Content-Type: application/json`, a `Cache-Control` and a stable `ETag`; actual: HTTP 501 NotImplemented.
3. Parse the style document's tile/sprite/glyph URL templates and GET /v2/tiles/<tileset-from-style>/3/4/5 — expect 200 with tile bytes, a stable `ETag` and the required `x-amz-geo-pricing-bucket` header; actual: 501.
4. Repeat the same GetTile request — expect byte-identical payload and the same `ETag` (proving the bytes came from persisted state, not a fresh random render).
5. GET /v2/tiles/no-such-tileset/3/4/5 — expect `ResourceNotFoundException` with HTTP 404; GET /v2/tiles/standard/999/0/0 — expect `ValidationException` with HTTP 400; actual: 501 for both.
6. GET /v2/styles/Standard/Light/Default/sprites/sprites.png and .../sprites.json — expect 200 with `image/png` and `application/json` respectively, and the JSON index describing icons present in the PNG; actual: 501.
7. GET /v2/glyphs/Amazon%20Ember%20Regular/0-255 — expect 200 with the glyph range payload and an `ETag`; actual: 501.
8. GET /v2/static/map.png?width=600&height=400&center=-123.1,49.3&zoom=10&style=Standard — expect 200 with image bytes, `Content-Type: image/png`, `ETag` and the required `x-amz-geo-pricing-bucket`; actual: 501.
9. GET /v2/static/map.png (no width/height, no framing) — expect `ValidationException` with HTTP 400; actual: 501.
