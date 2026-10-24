Issue-Severity: MEDIUM
Issue-Gap-Class: behavior
Issue-Commit: pending

# GetStaticMap accepts `PoliticalView`, `Language` and `CropLabels` but renders nothing for them

_Filed: 2026-10-22T01:30:00Z_
_Operation: GetStaticMap (service-com-amazonaws-geomaps-mapsservice) | Class: behavior_

> _The follow-up gap issue the Geo Maps bootstrap issue promised
> ("Operations and parameters outside this first slice ... may remain
> unimplemented and will be filed as individual gap issues later"), filed so the
> deferral stays tracked now that the bootstrap report has moved to
> `issues/things-that-now-work/`._

## Description

`GetStaticMap` renders the map image from the framed envelope, the stored
palette, the terrain field, the requested markers, the GeoJSON overlay and the
scale bar. Three modeled parameters are accepted and change nothing in the
bytes returned:

* **`Language`** (`@httpQuery("lang")`, len 2..35) — selects the language of the
  map's labels.
* **`CropLabels`** (`@httpQuery("crop-labels")`) — "it determines if the labels
  will be cropped out or kept on the edges" of the image.
* **`PoliticalView`** (`@httpQuery("political-view")`, pattern
  `^([A-Z]{2}|[A-Z]{3})$`) — renders a country's own view of a disputed
  boundary.

The first slice draws **no label text at all**, so `Language` and `CropLabels`
have nothing to act on: a request that sets them gets the same image as one that
does not, which is the correct image for a map with no labels, but not the image
the real service returns. `PoliticalView` is the sharper gap: the real service
draws boundaries differently per view, and this renderer draws no political
boundaries, so two requests that differ only in `political-view` are
byte-identical where AWS's differ.

`GetStyleDescriptor` does honour `PoliticalView` — it adds the `borders` layer
to the style document and records the view in `metadata["aws:political-view"]` —
so the gap is specific to the static-map renderer.

(The fourth deferred knob, `CompactOverlay`, is **not** part of this issue: it
asks for geometry to be drawn, so `GetStaticMap` refuses it with the modeled
`ValidationException` rather than answering with an image that leaves it out.)

## Steps to Reproduce

1. Start the emulator.
2. `GET /v2/static/map?width=600&height=400&center=77.5,34.0&zoom=6&political-view=IND`
3. `GET /v2/static/map?width=600&height=400&center=77.5,34.0&zoom=6&political-view=CHN`
4. Both answer 200 with byte-identical bodies and the same `ETag`. Expected: the
   two political views draw the disputed boundary differently, so the images (and
   their `ETag`s) differ.
5. The same holds for `lang=ja` versus `lang=en`, and for `crop-labels=true`
   versus `crop-labels=false`.

## Evidence

`services/service-com-amazonaws-geomaps-mapsservice/src/ops/get_static_map.rs`
builds `StaticMapView` from the framing, `label_size`, `pois` and
`scale-unit` only; `language`, `crop_labels` and `political_view` are read off
the input by the deserializer and never reach
`assets::render_static_map`. `services/service-com-amazonaws-geomaps-mapsservice/src/state/assets.rs`
draws the terrain, graticule, markers, POI lattice, overlay and scale bar — no
label text and no political boundaries.

## What a fix needs

Label rendering, which needs a text rasterizer and the glyph metrics the service
already stores for `GetGlyphs`; and a boundary layer in the terrain/vector model
whose geometry depends on the requested view, so that a political view selects
between alternative boundary lines. Until labels exist, `Language` and
`CropLabels` cannot be anything but accepted-and-vacuous.
