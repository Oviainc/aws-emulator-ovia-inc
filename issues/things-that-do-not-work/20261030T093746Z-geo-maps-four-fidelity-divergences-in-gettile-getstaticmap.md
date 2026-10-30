Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: geo-maps.tile-coordinate-out-of-range-rejected geo-maps.tile-invalid-tileset-pattern-rejected geo-maps.static-map-dimension-out-of-range-rejected geo-maps.static-map-mutually-exclusive-params-rejected geo-maps.glyphs-invalid-unicode-range-rejected geo-maps.tile-vector-basemap-returns-pbf-blob
Issue-Shape-Id: com.amazonaws.geomaps#MapsService
Issue-Key: geo-maps.tile-coordinate-out-of-range-rejected|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: geo-maps.tile-invalid-tileset-pattern-rejected|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: geo-maps.static-map-dimension-out-of-range-rejected|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: geo-maps.static-map-mutually-exclusive-params-rejected|run#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: geo-maps.glyphs-invalid-unicode-range-rejected|run#4|AssertionError|assert code == "404", f"expected a routing-level 404, got {code!r}"
Issue-Key: geo-maps.tile-vector-basemap-returns-pbf-blob|run#5|AssertionError|assert resp["ContentType"] == "application/x-protobuf", (
Issue-Commit: 5f287cca7598dba49dbfde37b90c5e553461c260

# geo-maps: four fidelity divergences in GetTile/GetStaticMap/GetGlyphs refusals and GetTile Content-Type

> _Note: this issue was identified against the code base at commit `5f287cca7598dba49dbfde37b90c5e553461c260`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTile, GetStaticMap, GetGlyphs

## Description
Four distinct net-new root causes in the geo-maps emulator, all grounded in failing behavioral checks. The emulator answers every one of these calls (18/24 geo-maps checks pass), so these are fidelity defects, not bootstrap gaps. No open issue covers any of them (list_existing_issues for 'geo-maps'/'geomaps' returns nothing).

=== Cause 1 of 4: constraint-violation ValidationException message uses a generic framework template instead of the Geo Maps wording ===
AWS: Smithy (services/service-com-amazonaws-geomaps-mapsservice/smithy-model/com-amazonaws-geomaps-mapsservice.json) constrains GetTileRequest$X/$Y with @length max 7 and @pattern \d+, GetTileRequest$Tileset with @pattern ^[-.\w]+$ and @length 1..100, and GetStaticMapRequest$Width/$Height with @range min 64 max 1400; each operation models ValidationException (HTTP 400). Recorded behavior: the refusal carries code ValidationException and a terse, member-scoped message of the form "'<Member>' must <constraint phrase>" — fixed text "'" + member name + "' must have length at most <max>", "'" + member name + "' must match regex <pattern>", "'" + member name + "' must not be greater than <max>". The variable parts are ONLY the member name as modeled (X, Tileset, Width) and the limit/pattern taken from the model (7, ^[-.\w]+$, 1400); no request value is echoed (X/Y/Z and Width are @sensitive).
Emulator: returns ValidationException with the shared boundary validator's generic template, e.g. "1 validation error detected: Value '99999999' at 'x' failed to satisfy constraint: Member must have length less than or equal to 7", "1 validation error detected: Value 'bad tileset!' at 'tileset' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[-.\w]+$", and "1 validation error detected: Value '2000' at 'width' failed to satisfy constraint: Member must have value less than or equal to 1400". Besides the wrong template it lowercases the member name and echoes the @sensitive value back to the client.
Why: clients and tests that branch on, log, or surface the service's validation message see a different sentence than AWS produces, and a @sensitive coordinate/dimension value leaks into the error text and into any log that keeps it.
Fix: the boundary constraint-validation layer as it serializes ValidationException for this service's operations (the centralized deserialize→validate step that runs before crate::ops::get_tile::handle / crate::ops::get_static_map::handle), so modeled length/pattern/range violations render the Geo Maps message shape with the modeled member name and no sensitive value.
Traces: geo-maps.tile-coordinate-out-of-range-rejected, geo-maps.tile-invalid-tileset-pattern-rejected, geo-maps.static-map-dimension-out-of-range-rejected.

=== Cause 2 of 4: GetStaticMap mutual-exclusivity refusal message wording ===
AWS: Smithy models BoundingBox, Center/Zoom and Radius as separate query parameters (bounding-box, center, zoom, radius) and documents BoundingBox as the two coordinate pairs that "become the view of the image", i.e. mutually exclusive with the Zoom/Center/Radius framing; the refusal is the modeled ValidationException (HTTP 400). Recorded behavior: the message is per-conflicting-member and reads "'Zoom' must not be set when 'BoundingBox' is set." — fixed text "' must not be set when '" / "' is set." with the two member names (as modeled, e.g. Zoom and BoundingBox; Radius in the Radius+BoundingBox case) as the variable parts, and a trailing period.
Emulator: returns ValidationException (correct code) with the message "BoundingBox cannot be used with Zoom or Radius" — one combined sentence naming the whole conflicting set rather than the offending member, and no trailing period.
Why: a client cannot tell from the message which parameter it must drop, and message assertions against AWS wording fail.
Fix: the GetStaticMap handler's cross-field exclusivity validation (services/service-com-amazonaws-geomaps-mapsservice/src/ops/get_static_map.rs, the BoundingBox/Zoom/Center/Radius conflict refusal) — emit one refusal naming the single member that must not be set alongside BoundingBox.
Traces: geo-maps.static-map-mutually-exclusive-params-rejected.

=== Cause 3 of 4: GetGlyphs turns a route-template mismatch into a ValidationException instead of a routing 404 ===
AWS: Smithy binds GetGlyphs to GET /v2/glyphs/{FontStack}/{FontUnicodeRange} and constrains FontUnicodeRange with @pattern ^[0-9]+-[0-9]+\.pbf$; GetGlyphs models NO errors at all (errors: [] — no ValidationException exists for this operation). Recorded behavior: a FontUnicodeRange that is not of the "<start>-<end>.pbf" shape does not match the route and comes back as a generic routing-level HTTP 404 with no modeled exception body, and no glyph data.
Emulator: refuses the same call with an error whose code is ValidationException — a code this operation does not model — rather than a routing 404.
Why: a client that distinguishes "no such glyph route/resource" (404, retry with a different range, often surfaced as a missing-asset condition) from "bad input" (400) is routed down the wrong branch, and botocore surfaces an exception shape that cannot occur on real GetGlyphs.
Fix: the GetGlyphs route/label matching in the request walk ahead of crate::ops::get_glyphs (services/service-com-amazonaws-geomaps-mapsservice/src/ops/get_glyphs.rs and the service's route table) — a label value that fails the modeled URI pattern must fall through to the generic 404 path, not into this service's constraint validator.
Traces: geo-maps.glyphs-invalid-unicode-range-rejected.

=== Cause 4 of 4: GetTile returns the wrong Content-Type for a vector (mvt) tileset ===
AWS: Smithy binds GetTileResponse$ContentType to the Content-Type header and documents Blob as "a vector tile in mvt or a raster tile in an image format". Recorded behavior: GetTile on vector.basemap at 0/0/0 returns 200 with Content-Type "application/x-protobuf" (fixed text; not request-derived) alongside a non-empty PricingBucket and an MVT protobuf body whose Tile.layers (field 3) submessages each carry name (field 1) and version (field 15).
Emulator: returns Content-Type "application/vnd.mapbox-vector-tile" for the same tileset, so the check fails before the body is inspected; whether the blob is well-formed MVT protobuf is therefore unverified and must be confirmed while fixing this.
Why: clients and caches that select a decoder by Content-Type (and SDK/stack code that asserts the protobuf media type) will not treat the payload as a vector tile.
Fix: the tileset records' content_type in this service's state (services/service-com-amazonaws-geomaps-mapsservice/src/state, consumed at services/service-com-amazonaws-geomaps-mapsservice/src/ops/get_tile.rs:239) for mvt-format tilesets.
Traces: geo-maps.tile-vector-basemap-returns-pbf-blob.

## Evidence
Behavioral suite: 18/24 geo-maps checks passed, 6 mismatched, 0 errored/skipped/unloadable. None marked ALREADY FILED or OWNED by.

Cause 1:
- geo-maps.tile-coordinate-out-of-range-rejected: GetTile(Tileset="vector.basemap", Z="1", X="99999999", Y="0") — expected code ValidationException with message "'X' must have length at most 7"; got code ValidationException with message "1 validation error detected: Value '99999999' at 'x' failed to satisfy constraint: Member must have length less than or equal to 7".
- geo-maps.tile-invalid-tileset-pattern-rejected: GetTile(Tileset="bad tileset!", Z="0", X="0", Y="0") — expected "'Tileset' must match regex ^[-.\w]+$"; got "1 validation error detected: Value 'bad tileset!' at 'tileset' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[-.\w]+$".
- geo-maps.static-map-dimension-out-of-range-rejected: GetStaticMap(FileName="map", Width=2000, Height=300, Style="Satellite", Center="-123.108,49.295", Zoom=10.0) — expected "'Width' must not be greater than 1400"; got "1 validation error detected: Value '2000' at 'width' failed to satisfy constraint: Member must have value less than or equal to 1400".

Cause 2:
- geo-maps.static-map-mutually-exclusive-params-rejected: GetStaticMap(FileName="map", Width=300, Height=300, Style="Satellite", BoundingBox="-123.17075,49.26959,-123.08125,49.31429", Zoom=10.0) — expected ValidationException message "'Zoom' must not be set when 'BoundingBox' is set."; got "BoundingBox cannot be used with Zoom or Radius".

Cause 3:
- geo-maps.glyphs-invalid-unicode-range-rejected: GetGlyphs(FontStack="Amazon Ember Regular", FontUnicodeRange="not-a-range") — expected a routing-level 404; got botocore Error Code 'ValidationException'. Model: GetGlyphs declares no errors, so ValidationException is not a modeled response for it.

Cause 4:
- geo-maps.tile-vector-basemap-returns-pbf-blob: GetTile(Tileset="vector.basemap", Z="0", X="0", Y="0") returned 200 with ContentType 'application/vnd.mapbox-vector-tile'; expected 'application/x-protobuf'. The MVT-protobuf body assertions were not reached.

## Steps to Reproduce
1. git_clone the repo and boot the emulator (run_behavioral_suite builds and boots it).
2. Point a botocore geo-maps client at the emulator endpoint.
3. Cause 1: call get_tile(Tileset="vector.basemap", Z="1", X="99999999", Y="0"); call get_tile(Tileset="bad tileset!", Z="0", X="0", Y="0"); call get_static_map(FileName="map", Width=2000, Height=300, Style="Satellite", Center="-123.108,49.295", Zoom=10.0). Inspect ClientError response["Error"]["Message"] — it is the generic "1 validation error detected: ..." template instead of the "'<Member>' must ..." Geo Maps wording, and it echoes the @sensitive value.
4. Cause 2: call get_static_map(FileName="map", Width=300, Height=300, Style="Satellite", BoundingBox="-123.17075,49.26959,-123.08125,49.31429", Zoom=10.0). The message is "BoundingBox cannot be used with Zoom or Radius" instead of "'Zoom' must not be set when 'BoundingBox' is set.".
5. Cause 3: call get_glyphs(FontStack="Amazon Ember Regular", FontUnicodeRange="not-a-range"). The failure surfaces as error code ValidationException instead of a routing-level HTTP 404 with no modeled body. Control: get_glyphs(FontStack="Amazon Ember Regular", FontUnicodeRange="0-255.pbf") still succeeds.
6. Cause 4: call get_tile(Tileset="vector.basemap", Z="0", X="0", Y="0") and read resp["ContentType"] — 'application/vnd.mapbox-vector-tile' instead of 'application/x-protobuf'; then decode resp["Blob"] as a Mapbox Vector Tile and confirm Tile.layers each carry name and version.
7. Re-replay the six check ids in trace_ids to confirm the fix.
