Issue-Source: trace-sweep-divergence
Issue-Trace: geo-places-geocode-include-place-types-filter
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: geo-places-geocode-include-place-types-filter geo-places-geocode-query-components-structured-search geo-places-reverse-geocode-political-view-changes-country-code geo-places-search-text-max-results-enforced
Issue-Shape-Id: com.amazonaws.geoplaces#PlacesService
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].PlaceId
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$.ResultItems[*].PlaceId
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.AddressNumber
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.District
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.Label
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.PostalCode
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.Street
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.StreetComponents[*].BaseName
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.StreetComponents[*].Type
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Distance
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Position[*]
Issue-Commit: a32a3e20a1235e995b8b82a77105d44e9a1e2c6f

# geo-places serves a synthesized place catalog: PlaceIds and SearchText nearby-POI results diverge from AWS

> _Note: this issue was identified against the code base at commit `a32a3e20a1235e995b8b82a77105d44e9a1e2c6f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Geocode

## Description
Three net-new wire divergences in geo-places, all of them 200 -> 200 bodies whose place records do not match AWS's place data. They share one root cause: this crate holds AWS's place data in its own seeded catalog (services/service-com-amazonaws-geoplaces-placesservice/src/state.rs module doc: "That data therefore lives in this service's state, seeded once ... with a deterministic catalog of places"), and the handles and the point-of-interest records that catalog hands out are emulator-authored rather than AWS's. Every other member of these responses matched, so the divergence is confined to the catalog's identity values and to one neighbourhood's POI records.

Cause 1 of 3 — Geocode result PlaceId is a hash of the catalog record, not AWS's place handle.
AWS: Smithy: Geocode (POST /v2/geocode, 200) -> GeocodeResponse$ResultItems: GeocodeResultItem$PlaceId is @required SensitiveString @length(0, 500), documented only as "The PlaceId of the place result". The model states nothing about its content, so this is recorded behavior only: the recording shows a PlaceId that is AWS's own catalog handle for the matched place, carrying the service's version/length header (the first eight characters are shared with the emulator's value) followed by a dataset-derived payload. The same handle is the input of GET /v2/place/{PlaceId}, so a client may persist it, de-duplicate on it and hand it back later.
Emulator: for the very same matched place — Title, Address, Position and PlaceType all compared equal — it answers a different PlaceId. synthesize_place_id() in src/state.rs derives the payload by SHA-256 over "{title}|{lon:.6}|{lat:.6}" (state.rs, the PLACE_ID_VERSION header + PLACE_ID_PAYLOAD_BYTES expansion, base64url via place_id_token()), so the handle is stable across runs but is not the handle AWS issues for that place. Both of the fixtures in this cluster (include-place-types-filter and query-components-structured-search) diverge on $.ResultItems[0].PlaceId only.
Why: a PlaceId is the one value in a Places response a client is expected to keep. Any id captured against AWS (in a test fixture, a cache, a database column, a de-duplication key) does not resolve or compare equal here, and conversely ids captured here are meaningless against AWS, so round-tripping Geocode -> GetPlace works only when both halves run against the same emulator.
Fix: the catalog seed and its handle derivation in this service's state — the place records should carry AWS's recorded PlaceId for the places the catalog reproduces instead of synthesizing one from Title and Position (see Traces for the file sites).

Cause 2 of 3 — the same synthesized-handle divergence on ReverseGeocode, on both steps of the political-view fixture.
AWS: Smithy: ReverseGeocode (POST /v2/reverse-geocode, 200) -> ReverseGeocodeResponse$ResultItems; ReverseGeocodeResultItem$PlaceId is @required, same shape and same opaque-handle contract as Geocode's. Recorded behavior only: the recording returns AWS's catalog handle for the place at the queried position, and it returns the SAME place (hence the same handle) whether or not PoliticalView is supplied, because PoliticalView changes only the country code reported for the place.
Emulator: both steps of the fixture (the plain reverse-geocode and the PoliticalView-qualified repeat) answer a PlaceId that is not the recorded one, again as the only diverging member of the body: $.ResultItems[0].PlaceId. The id comes from the same synthesize_place_id() derivation as Cause 1, so a fix to the derivation/seed must cover the ReverseGeocode projection too.
Why: cross-operation identity breaks the same way as in Cause 1, and because the divergence reproduces on the political-view step as well, a client cannot use the handle to recognise that the plain and politically-qualified views describe one and the same place as recorded.
Fix: same component as Cause 1 — the catalog's PlaceId source, consumed by the ReverseGeocode handler's result projection.

Cause 3 of 3 — SearchText returns different point-of-interest records and derived distances for the lower-ranked results.
AWS: Smithy: SearchText (POST /v2/search-text, 200) -> SearchTextResponse$ResultItems of SearchTextResultItem, whose Position is "the position in World Geodetic System (WGS 84) format: [longitude, latitude]", Distance is "the distance in meters from the QueryPosition" (@range 0..4294967295, @default 0) and Address.Label/Street/StreetComponents/PostalCode/District/AddressNumber describe the place that result names. Recorded behavior only for the data itself: for this chain-POI query AWS returns, after the top hit, specific nearby branches of the chain ordered by distance from the bias position, with each item's Address, Position and Distance all describing one real branch and Distance consistent with that item's Position.
Emulator: items 1 and 2 of the result list describe different places than the recording. Item 1's Position and Distance are both shifted (Position differs in both ordinates; Distance differs by order ~80 m). Item 2 is a DIFFERENT branch of the chain altogether — every address member moves together (Address.Label, Street, StreetComponents[0].BaseName/Type, AddressNumber, District, PostalCode) along with its Position and Distance, i.e. the catalog's nearby-POI set for this locality does not contain the branches AWS returns, so the ranked list past the first hit is populated from different records. The response status (200) and the result-list shape, length and MaxResults handling all matched.
Why: a client that reads anything past ResultItems[0] — showing the nearest branches, picking one by distance, asserting on an address or a postal code, or computing a route to a result's Position — gets places that do not exist at those coordinates in AWS's data, and gets distances that do not agree with the recorded ranking. Address and Distance are the values this operation exists to supply, so this is a wrong answer rather than a cosmetic difference.
Fix: the SearchText handler's result projection and the nearby-POI records of the seeded catalog it ranks over (the chain/branch entries for this locality), so that the ordered result list and each item's Address, Position and Distance agree with the recorded data.

Note: no open geo-places issue covers any of these; the nearest neighbour is the unrelated open issue 20261103T183228Z-geo-routes-route-synthesis-ignores-the-road-network.md for geo-routes. All three causes must be closed before submitting.

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, steps_total=5, steps_passed=0, steps_mismatched=5, divergence_count=3 — every divergence is service=geo-places, verdict=mismatch, expected_status=200, actual_status=200, stub_path empty, owned_by empty.

Cause 1 (Geocode, fixture geo-places-geocode-include-place-types-filter step task#1, divergence_key service=geo-places|Geocode|status=200->200|keys=$.ResultItems[0].PlaceId; affected_count=2, also geo-places-geocode-query-components-structured-search#task#1):
  $.ResultItems[0].PlaceId: value_mismatch — expected 'AQAAADgAS4dlANlTcJAwxzkQeY4KmDsNP_-mbI9KgIrdgBJy84gfbsSLbZGuZFNi0t5RW4yDS2sWLBA3xaF0T_lgjPYHVrFMDllob_vUqyz9hUkyF68F5Y5-4DE66Q', got 'AQAAADgADv-iRi6gce5gCWxg37T01jzsVtSGP4Xnf8oZpDhxzQHtWd4azGvJOGGmD3MZQmYG5sqgDWcGBXG-XIOSDYH6BJmnbOMYPBg_0a8om03ygon9-XSt7PViFg'.
  No other member of the body diverged; the shared 'AQAAADgA' prefix is the version/length header both sides emit, the differing remainder is the handle payload.

Cause 2 (ReverseGeocode, fixture geo-places-reverse-geocode-political-view-changes-country-code step task#1, divergence_key service=geo-places|ReverseGeocode|status=200->200|keys=$.ResultItems[0].PlaceId; affected_count=2 — task#1 and task#2 of that fixture):
  $.ResultItems[0].PlaceId: value_mismatch — expected 'AQAAADgApjjt2WZrCL3y4TIpZfYohfzY_CnvR89lM4pJoXwZmFlTtTdjHN-fQ2xZcNKgU6zd_-jLCssEJSvYtSiIjj-Tgop4nlezkfb8Zt8Lf5EW7b6IVOG2E_UjKA', got 'AQAAADgANOurlXZm0cmDbzwiVQ8rrS8KRyz3zd6N57EB0PiJGaGyl4DsdPgBLd6f2iQYt2VAPIsnjQgu859k9JLBkUShy1IG7bt1RK3IyXiTluKR12rr7h7Dhfb8HA'.

Cause 3 (SearchText, fixture geo-places-search-text-max-results-enforced step task#1, divergence_key service=geo-places|SearchText|status=200->200 over 13 keys; affected_count=1):
  $.ResultItems[1].Position[0]: expected -122.33695, got -122.33581
  $.ResultItems[1].Position[1]: expected 47.62287, got 47.62276
  $.ResultItems[1].Distance: expected 962, got 1042
  $.ResultItems[2].Address.Label: expected 'STARBUCKS, 2011 7th Ave, Seattle, WA 98121-2601, United States', got 'STARBUCKS, 1912 Pike Pl, Seattle, WA 98101, United States'
  $.ResultItems[2].Address.District: expected 'Belltown', got 'Downtown'
  $.ResultItems[2].Address.PostalCode: expected '98121-2601', got '98101'
  $.ResultItems[2].Address.Street: expected '7th Ave', got 'Pike Pl'
  $.ResultItems[2].Address.StreetComponents[0].BaseName: expected '7th', got 'Pike'
  $.ResultItems[2].Address.StreetComponents[0].Type: expected 'Ave', got 'Pl'
  $.ResultItems[2].Address.AddressNumber: expected '2011', got '1912'
  $.ResultItems[2].Position[0]: expected -122.33822, got -122.34062
  $.ResultItems[2].Position[1]: expected 47.61523, got 47.6101
  $.ResultItems[2].Distance: expected 1016, got 1327
  The whole of ResultItems[2]'s address moves together, i.e. a different POI record, and each item's Distance moves with its Position.

Code sites backing the Fix lines: services/service-com-amazonaws-geoplaces-placesservice/src/state.rs lines 5-16 (the seeded catalog is this service's stand-in for AWS place data), lines 2124-2136 (place_id_token, base64url) and lines 2138-2163 (synthesize_place_id: SHA-256 over "{title}|{lon:.6}|{lat:.6}" after the PLACE_ID_VERSION header).

Values quoted above are the recorded examples; the requirement is that a result's PlaceId be the handle AWS issues for that place and that each result item's Address/Position/Distance describe the place AWS ranks at that index, not these literal strings and numbers.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it; confirm /__admin/health answers 200.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus) — all 5 steps of the 4 geo-places fixtures mismatch, clustering into the 3 divergences above.
3. Cause 1: replay fixture geo-places-geocode-include-place-types-filter step task#1 (POST /v2/geocode) and compare $.ResultItems[0].PlaceId with the recorded response; repeat with geo-places-geocode-query-components-structured-search#task#1.
4. Cause 2: replay fixture geo-places-reverse-geocode-political-view-changes-country-code steps task#1 and task#2 (POST /v2/reverse-geocode) and compare $.ResultItems[0].PlaceId on both.
5. Cause 3: replay fixture geo-places-search-text-max-results-enforced step task#1 (POST /v2/search-text) and diff $.ResultItems[1] and $.ResultItems[2] — Position, Distance and the whole Address sub-tree of item 2 — against the recorded response.
6. All three causes must replay clean before closing.
