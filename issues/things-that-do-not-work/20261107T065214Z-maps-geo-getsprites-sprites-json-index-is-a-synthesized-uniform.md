Issue-Source: trace-sweep-divergence
Issue-Trace: geo-maps-sprites-style-color-variants
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: geo-maps-sprites-style-color-variants geo-maps-vector-basemap-style-workflow
Issue-Shape-Id: com.amazonaws.geomaps#MapsService
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-AE:E-route
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-AU:MR
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-AU:NH
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-AU:S
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-AU:T
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-CA:transcanada
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-CN:expressway
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-CN:expressway-long
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-CR:CR
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-CR:national
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-DE:BAB
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-HU:national
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-JP:JP
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-JP:national
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-JP:prefectural
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-KR:expressway
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-KR:expressway-long
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-KR:national
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-NL:binnenstedelijke_ring
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-PH:E
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-PK:motorway
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-PK:national
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-SA:national
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TR:motorway
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TR:motorway-long
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TW:expressway
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TW:freeway
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TW:provincial
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-VE:T
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-ch:national
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-cy:national
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-md:national
Issue-Related: 20261102T030925Z-maps-geo-serves-synthesized-glyph-and-sprite-assets.md
Issue-Commit: d4bc6001d068bdcbd97a8ad753f1dc3866a0195e

# GetSprites sprites.json index is a synthesized uniform-grid atlas instead of the published per-style index

> _Note: this issue was identified against the code base at commit `d4bc6001d068bdcbd97a8ad753f1dc3866a0195e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSprites

## Description
GetSprites served with FileName=sprites.json returns the sprite INDEX document (the companion of the sprite raster). The emulator does not reproduce the published index: it computes one at request time from a uniform grid, so every icon's footprint and its position in the atlas are emulator-invented, the packing wraps onto a second row the published sheet does not use, and part of the published icon-name set is absent or differently named. The same generated index is emitted for every (Style, ColorScheme, Variant), so the defect is not specific to one style. Two divergence clusters are bundled here: they are the SAME synthesizer but two different published indexes, so a fix that hardcodes one style's index still fails the other.

NOTE ON AN ADJACENT OPEN ISSUE: open issue 20261102T030925Z-maps-geo-serves-synthesized-glyph-and-sprite-assets.md discusses this same sprites.json behaviour in prose, but its replay pointer does NOT reach either step filed here — its Issue-Trace/Issue-Step is geo-maps-glyphs-fontstack-unicode-ranges#task#1 and its Issue-Trace-Ids are bare fixture ids with no '#step' half, so neither geo-maps-sprites-style-color-variants#task#3 nor geo-maps-vector-basemap-style-workflow#task#3 is replayed when that issue is verified. The replay accordingly reports owned_by=[] for both clusters. This issue exists to carry those two replay keys so the fix is actually gated on them.

=== Cause 1 of 2: sprites.json icon geometry is a uniform-grid packing, not the published atlas geometry (style/colour-variant fixture) ===
AWS: Smithy (services/service-com-amazonaws-geomaps-mapsservice/smithy-model/com-amazonaws-geomaps-mapsservice.json) binds GetSprites to GET /v2/styles/{Style}/{ColorScheme}/{Variant}/sprites/{FileName}, method GET, code 200, with FileName @pattern ^sprites(@2x)?\.(png|json)$ and GetSpritesResponse$Blob as the @httpPayload; GetSprites models NO errors, and the model says nothing about the payload's internal structure — so for the document body this is recorded behavior only. Recorded behavior: 200 with a flat JSON object keyed by published icon name, each value carrying that icon's width, height, x, y (and pixelRatio) inside the published sheet. The geometry is the published atlas's own packing: icons keep their natural, non-square footprints and the recorded combination places this whole group on the first row (y=0). The recorded values are properties of a static published asset keyed only by the four URI labels — identical on every call, in every region and account — not per-run values.
Emulator: 200, but with an index it derives from a uniform grid. It forces a handful of fixed square footprints onto every icon where the published icons are non-square (recorded 22x20 and 20x20 icons come back 21x21; recorded 5x5 and 6x6 city dots come back 12x12; the *_lowrank dots come back 8x8), it advances x by its own running offset so positions are shifted by small constants or by whole cells, and then it WRAPS to a second row at y=32 for icons the published index keeps at y=0 (those icons come back with x restarting from 0 and y=32). It also diverges on the icon NAME set: whole-node divergences are reported for the $.shield-AE:*, $.shield-AU:*, $.shield-CA:*, $.shield-CN:*, $.shield-CR:*, $.shield-DE:*, $.shield-HU:*, $.shield-JP:*, $.shield-KR:*, $.shield-NL:*, $.shield-PH:*, $.shield-PK:*, $.shield-SA:*, $.shield-TR:*, $.shield-TW:*, $.shield-VE:*, $.shield-ch:*, $.shield-cy:* and $.shield-md:* entries, i.e. those names are absent from or renamed in the emulator's index. Status and HTTP shape match (200 -> 200); the divergence is entirely in the document.
Why: this index is the only thing that tells a renderer where an icon lives in the sheet. Wrong width/height crops neighbouring pixels into every symbol; a shifted or second-row x/y points at atlas coordinates the published sheet never uses, so POI markers, one-way arrows and road shields draw garbage or nothing; and an icon name the index does not carry makes the style's icon-image lookup fail to resolve silently. A client that validates the index against the published asset, or that pairs it with a real published sprite raster, is broken outright.
Fix: the GetSprites handler's sprites.json branch and the sprite-index asset in this service's state — the handler must serve the stored published index document verbatim (names plus per-icon width/height/x/y/pixelRatio) keyed by (Style, ColorScheme, Variant, FileName), instead of computing an atlas layout at request time.
Traces: geo-maps-sprites-style-color-variants#task#3 (this bundle's representative). Component: services/service-com-amazonaws-geomaps-mapsservice/src/ops/get_sprites.rs and services/service-com-amazonaws-geomaps-mapsservice/src/state.

=== Cause 2 of 2: the same synthesized index is returned for the vector-basemap workflow's style, whose published index has a different icon set ===
AWS: same GetSprites binding and the same recorded-behavior-only payload contract, for the (Style, ColorScheme, Variant) combination the vector-basemap workflow requests. Recorded behavior: that combination's published index is a DIFFERENT document from Cause 1's — it carries its own icon-name set and its own geometry (for example it carries a dash-and-camel-case TransCanada shield entry with its own width/height/x/y where the other combination carries a colon-namespaced shield name, and it gives truck-parking and stadium their own natural footprints).
Emulator: 200, and emits the IDENTICAL generated uniform-grid index as Cause 1 — same forced square footprints, same running-x offsets, same y=32 row wrap, same missing/renamed colon-namespaced shield entries. Because the expected document differs between the two combinations while the emulator's response does not, the replay reports this as its own divergence cluster.
Why: same client impact as Cause 1, and it establishes that the index is not keyed on the style at all: every style/colour-scheme/variant the emulator serves gets the same invented atlas, so no client can rely on the index for any style. It also means a fix that stores only one combination's index leaves this step failing.
Fix: the same GetSprites sprites.json branch, but the index must be stored and looked up PER (Style, ColorScheme, Variant) in this service's state rather than from a single shared source.
Traces: geo-maps-vector-basemap-style-workflow#task#3. Component: services/service-com-amazonaws-geomaps-mapsservice/src/ops/get_sprites.rs and services/service-com-amazonaws-geomaps-mapsservice/src/state.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, bootstrap_signal=false, 4 fixtures replayed, steps_total=12, steps_passed=0, steps_mismatched=12, 7 divergence clusters, score_pct=0.0. All seven clusters are service=maps.geo. No cluster carries a stub_path. Five clusters report a non-empty owned_by (the GetGlyphs/GetSprites-raster cluster pair and the three GetStyleDescriptor clusters) and are excluded. The two clusters below report owned_by=[].

Cause 1 — fixture geo-maps-sprites-style-color-variants, step task#3, operation GetSprites, verdict mismatch, expected_status 200 / actual_status 200, affected_count 1. Reported field-level divergences (expected vs actual), all inside the sprites.json index document:
  $.bicycle.width 22 vs 21; $.bicycle.height 20 vs 21
  $.bus.width 20 vs 21; $.bus.height 20 vs 21; $.bus.x 138 vs 137
  $.book.x 117 vs 116
  $.citydot.width 5 vs 12; $.citydot.height 5 vs 12; $.citydot.x 317 vs 305
  $.citydot-open.width 6 vs 12; $.citydot-open.height 6 vs 12; $.citydot-open.x 305 vs 317
  $.citydot_lowrank.width 5 vs 8; $.citydot_lowrank.height 5 vs 8; $.citydot_lowrank.x 322 vs 337
  $.citydot-open_lowrank.width 6 vs 8; $.citydot-open_lowrank.height 6 vs 8; $.citydot-open_lowrank.x 311 vs 329
  running-x shifts: $.cocktail.x 327 vs 345; $.coffee.x 348 vs 366; $.congestion.x 369 vs 387; $.construction.x 401 vs 419; $.delivery-entrance.x 433 vs 451; $.doctor.x 454 vs 472
  second-row wrap (published y=0, emulator y=32): $.education.x 475 vs 0 / .y 0 vs 32; $.entrance.x 496 vs 21 / .y 0 vs 32 (plus .width 20 vs 21, .height 20 vs 21); $.facilities.x 516 vs 42 / .y 0 vs 32; $.fast-food.x 537 vs 63 / .y 0 vs 32; $.ferry.x 558 vs 84 / .y 0 vs 32; the same pattern continues for funicular, game, gas-station, government, grocery, gym, harbour, hospital, information, light-rail, loading-zone, marker*, metro, money, monorail, museum, nature, nightclub, oneway, other, outdoor, park, parking, pharmacy, place-of-worship, police, pub, rail-yard, restaurant, road_closure, salon, shopping, ski, stadium, swimming, taxi, theater, toll-booth, train, truck-parking, truck-services, truck-weigh-station
  whole-entry (name-set) divergences: $.shield-AE:E-route, $.shield-AU:MR, $.shield-AU:NH, $.shield-AU:S, $.shield-AU:T, $.shield-CA:transcanada, $.shield-CN:expressway, $.shield-CN:expressway-long, $.shield-CR:CR, $.shield-CR:national, $.shield-DE:BAB, $.shield-HU:national, $.shield-JP:JP, $.shield-JP:national, $.shield-JP:prefectural, $.shield-KR:expressway, $.shield-KR:expressway-long, $.shield-KR:national, $.shield-NL:binnenstedelijke_ring, $.shield-PH:E, $.shield-PK:motorway, $.shield-PK:national, $.shield-SA:national, $.shield-TR:motorway, $.shield-TR:motorway-long, $.shield-TW:expressway, $.shield-TW:freeway, $.shield-TW:provincial, $.shield-VE:T, $.shield-ch:national, $.shield-cy:national, $.shield-md:national

Cause 2 — fixture geo-maps-vector-basemap-style-workflow, step task#3, operation GetSprites, verdict mismatch, expected_status 200 / actual_status 200, affected_count 1, owned_by=[]. Identical actual document (same 21x21 / 12x12 / 8x8 forced footprints, same running-x offsets, same y=32 wrap, same missing colon-namespaced shield entries) but a different EXPECTED index: this combination's published index carries $.shield-CA-TransCanada.width/.height/.x/.y and gives $.truck-parking and $.stadium their own width/height, where Cause 1's expects $.shield-CA:transcanada and different stadium/truck-parking geometry. Hence a separate cluster with a separate divergence_key.

Model evidence (Smithy, services/service-com-amazonaws-geomaps-mapsservice/smithy-model/com-amazonaws-geomaps-mapsservice.json): GetSprites -> GET /v2/styles/{Style}/{ColorScheme}/{Variant}/sprites/{FileName}, code 200 (defaulted), readonly; input FileName (@httpLabel, required, @pattern ^sprites(@2x)?\.(png|json)$), Style/ColorScheme/Variant (@httpLabel, required, enums); output Blob (@httpPayload) plus Content-Type, Cache-Control and ETag headers; errors: none. The index document's internal structure is not modelled, so its contents are asserted from the recording only.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout (the geo-maps service answers on the data listener; /__admin/health returns 200).
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus). 12/12 steps mismatch, 7 clusters, all service=maps.geo.
3. Cause 1: replay fixture geo-maps-sprites-style-color-variants step task#3 — a GetSprites request GET /v2/styles/{Style}/{ColorScheme}/{Variant}/sprites/sprites.json for the style/colour-variant that fixture exercises. The emulator answers 200 with a JSON index whose per-icon width/height are forced to 21x21 / 12x12 / 8x8, whose x values are shifted from the published offsets, and whose later icons sit on a y=32 second row; several colon-namespaced $.shield-XX:* entries are absent or renamed. Diff it against the recorded index: the published document keeps each icon's natural footprint (e.g. 22x20, 20x20, 5x5, 6x6) and keeps this group on y=0.
4. Cause 2: replay fixture geo-maps-vector-basemap-style-workflow step task#3 — the same GetSprites sprites.json call for that workflow's style. The emulator returns the byte-identical generated index, while the recorded index for that combination differs (it carries shield-CA-TransCanada with its own geometry and its own truck-parking/stadium footprints), proving the index is not keyed on (Style, ColorScheme, Variant).
5. Both steps must pass before this issue is closed: fixing only one style's index leaves the other failing.
