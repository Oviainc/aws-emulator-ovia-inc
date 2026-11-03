Issue-Source: trace-sweep-divergence
Issue-Trace: geo-places-geocode-include-place-types-filter
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: geo-places-geocode-include-place-types-filter geo-places-geocode-query-components-structured-search geo-places-reverse-geocode-political-view-changes-country-code geo-places-search-text-max-results-enforced
Issue-Shape-Id: com.amazonaws.geoplaces#PlacesService
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].MapView[*]
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].PlaceId
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].Position[*]
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$.ResultItems[*].MapView[*]
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$.ResultItems[*].PlaceId
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.AddressNumber
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.District
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.Label
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.PostalCode
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.Street
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.StreetComponents[*].BaseName
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.StreetComponents[*].Suffix
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.StreetComponents[*].Type
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Categories[*]
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Distance
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Position[*]
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Title
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# geo-places result items diverge from the recorded service: synthesized PlaceId, computed MapView, substituted POI records

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Geocode

## Description
Every step of the geo-places wire corpus (5/5 steps, 4 fixtures, score 0%) answers HTTP 200 with a structurally valid, modeled-conformant body whose RESULT-ITEM VALUES are not the ones the service returns. The emulator does not report AWS place data at all: it projects every response out of a seeded in-crate catalog (services/service-com-amazonaws-geoplaces-placesservice/src/state.rs, seed_catalog) and derives the identity and geometry members arithmetically from that catalog. Three distinct root causes, all in the same crate, all reached by Geocode, ReverseGeocode, SearchText (and by every other operation that projects a result item: Autocomplete, SearchNearby, GetPlace, Suggest).

Cause 1 of 3 - PlaceId is a hex digest instead of the service's opaque handle (Geocode, ReverseGeocode, SearchText, GetPlace, Autocomplete, SearchNearby).
AWS: Smithy GeocodeResultItem$PlaceId / SearchTextResultItem$PlaceId -> SensitiveString, @required, @length(min 0, max 500), documented only as "The PlaceId of the place result"; the model constrains nothing further, so this is recorded behavior only. Every recording in the corpus returns a long opaque provider handle built from the base64url alphabet and beginning with the provider's fixed version prefix 'AQAAAD' - e.g. 'AQAAADgAS4dlANlTcJAwxzkQeY4KmDsNP_-mbI9KgIrdgBJy84gfbsSLbZGuZFNi0t5RW4yDS2sWLBA3xaF0T_lgjPYHVrFMDllob_vUqyz9hUkyF68F5Y5-4DE66Q' (Geocode) and 'AQAAADgApjjt2WZrCL3y4TIpZfYohfzY_CnvR89lM4pJoXwZmFlTtTdjHN-fQ2xZcNKgU6zd_-jLCssEJSvYtSiIjj-Tgop4nlezkfb8Zt8Lf5EW7b6IVOG2E_UjKA' (ReverseGeocode). The fixed part of the token is the leading version prefix and the alphabet/length class (~100+ chars, base64url with '-' and '_'); the remaining characters are per-place payload, so a specific id value is never itself the requirement.
Emulator: synthesize_place_id (state.rs:1956-1968) returns the first 40 characters of an UPPERCASE HEX SHA-256 of "<title>|<lon:.6>|<lat:.6>" - '0FBBD1D689F1BF00A91B3C62DF369B6870A8CA91' for the Geocode step and 'B41F842082DF20C6354A8B9FF319E00CBE05CE68' for the ReverseGeocode step. Wrong alphabet (hex, no base64url characters), wrong length class (40 vs ~106), no version prefix.
Why: PlaceId is the handle clients persist, log, de-duplicate on and pass back to GetPlace. A client that validates or round-trips the handle (length bucket, charset, provider prefix) rejects the emulator's id, and recorded-vs-replayed comparisons of a stored id never match even for the same place.
Fix: the PlaceId minting function in the service's state (state.rs synthesize_place_id) - it must emit a token in the provider's opaque base64url form with the recorded version prefix and length class, while staying deterministic so one operation's id still resolves in another.

Cause 2 of 3 - MapView is a symmetric box computed from a fixed per-granularity extent, not the result's real bounding box, and is emitted at full double precision (Geocode, ReverseGeocode, and every other result item carrying MapView).
AWS: Smithy GeocodeResultItem$MapView -> BoundingBox, list<Double> @length(4,4), documented as "the bounding box enclosing the geometric shape (area or line) that an individual result covers... [{westward lng}, {southern lat}, {eastward lng}, {northern lat}]". The recordings show a box that is ASYMMETRIC about Position (it encloses the real feature geometry, not a square centred on the point) and whose members are rounded to 5 decimal places - e.g. the Geocode step's locality box [-122.43613, 47.49558, -122.23648, 47.73416] around Position [-122.32945, 47.60357], and the ReverseGeocode step's [33.52692, 45.34303, 34.12277, 45.80953].
Emulator: map_view_around (state.rs:310-322) builds [lon-d, lat-d, lon+d, lat+d] from one of seven hard-coded extents (state.rs:3090-3111: POINT_EXTENT_M 150, DISTRICT 3000, LOCALITY 12000, RURAL_DISTRICT 40000, SUB_REGION 60000, REGION 350000, COUNTRY 2000000) converted through a spherical-earth constant, so the box is always exactly centred on Position and always the same size for every place of a given PlaceType. It is serialized unrounded: [-122.49011830536023, 47.49591355635305, -122.17000569463977, 47.71175044364694] for the Geocode step and [33.3441957500934, 45.35447185451018, 34.3745842499066, 46.073928145489816] for the ReverseGeocode step.
Why: MapView is what a client uses to frame a map or to decide whether a point falls inside a result. A synthetic square of the wrong size and centre frames the wrong viewport, and because the emulator's own ReverseGeocode candidate test (PlaceRecord::covers / Catalog::at_position, state.rs:1603-1606, 2964-) is 'does the point fall inside this MapView', the fabricated extents also change WHICH places a position resolves to and what Distance they report. The 15-significant-digit doubles additionally make every value differ from the recorded 5-decimal form even where the box would otherwise agree.
Fix: the MapView projection in the service's state - the catalog records must carry a real per-place bounding box (and it must be rounded to the service's 5-decimal coordinate precision on serialization) instead of map_view_around deriving one from a PlaceType-keyed extent constant.

Cause 3 of 3 - the seeded POI catalog is not the service's place data, so SearchText answers with different places (SearchText, and any operation reading the same records).
AWS: Smithy SearchTextResultItem$Title/Address/Position/Distance (Title @required; Distance = "the distance in meters from the QueryPosition"); the content of the place data is recorded behavior only. For the corpus's Seattle coffee query the service returns, in rank order, the branded POIs 'STARBUCKS' at '500 Mercer St, Seattle, WA 98109-4654' (District 'Lower Queen Anne', Position [-122.34726, 47.62472], Distance 494) and 'STARBUCKS' at '442 Terry Ave N, Seattle, WA 98109-5210' (District 'South Lake Union', StreetComponents BaseName 'Terry', Type 'Ave', Suffix 'N'). Note the general properties of the recorded data: POI titles come through in the provider's upper-case brand form, postal codes are ZIP+4 ('98109-4654'), and StreetComponents carries a directional Suffix when the street name has one.
Emulator: the corresponding records are hand-seeded with different values (state.rs:3757-3793: 'Starbucks' at 1912 Pike Pl 98101 Downtown [-122.340622, 47.6101], 'Starbucks' at 102 Pike St 98101, 'Starbucks' at 701 5th Ave 98104), so the replayed items diverge in Title casing ('Starbucks' vs 'STARBUCKS'), Address.Label, Street, StreetComponents.BaseName/Type, a missing StreetComponents.Suffix, AddressNumber, District ('Downtown' for all of them vs the real neighbourhoods), PostalCode (5-digit vs ZIP+4), Position, Distance (1327 vs 494) and one Categories entry.
Why: a client asserting on the places a query returns - a branded title, a ZIP+4, a street suffix, a neighbourhood, a ranked first hit or a distance - sees different data than the service returns, and the ordering/MaxResults behavior the fixture exercises is evaluated over the wrong candidate set.
Fix: the seeded place catalog in the service's state (seed_catalog and the street_in/AddressRecord helpers it builds records with) - the records the corpus touches must carry the recorded titles, ZIP+4 postal codes, districts, street components including the directional suffix, and positions, so text search ranks and reports the same places.

Traces (examples of the above, not requirements in themselves): geo-places-geocode-include-place-types-filter#task#1 and geo-places-geocode-query-components-structured-search#task#1 (Cause 1 + Cause 2, Geocode); geo-places-reverse-geocode-political-view-changes-country-code#task#1 and #task#2 (Cause 1 + Cause 2, ReverseGeocode); geo-places-search-text-max-results-enforced#task#1 (Cause 3, SearchText). No geo-places issue is currently open, so nothing else covers these.

## Evidence
Replay of trace set simparity-wire-aws-v1: steps_total 5, steps_passed 0, steps_mismatched 5, score_pct 0.0, emulator_up true, bootstrap_signal false. All three divergence clusters are verdict=mismatch with expected_status 200 and actual_status 200 - the divergence is entirely in the response body.

Cluster 1 - fixture geo-places-geocode-include-place-types-filter, step task#1, operation Geocode (also affects geo-places-geocode-query-components-structured-search#task#1):
  $.ResultItems[0].PlaceId: expected 'AQAAADgAS4dlANlTcJAwxzkQeY4KmDsNP_-mbI9KgIrdgBJy84gfbsSLbZGuZFNi0t5RW4yDS2sWLBA3xaF0T_lgjPYHVrFMDllob_vUqyz9hUkyF68F5Y5-4DE66Q', got '0FBBD1D689F1BF00A91B3C62DF369B6870A8CA91'
  $.ResultItems[0].Position[0]: expected -122.32945, got -122.330062
  $.ResultItems[0].Position[1]: expected 47.60357, got 47.603832
  $.ResultItems[0].MapView[0..3]: expected [-122.43613, 47.49558, -122.23648, 47.73416], got [-122.49011830536023, 47.49591355635305, -122.17000569463977, 47.71175044364694]

Cluster 2 - fixture geo-places-reverse-geocode-political-view-changes-country-code, step task#1, operation ReverseGeocode (also affects #task#2):
  $.ResultItems[0].PlaceId: expected 'AQAAADgApjjt2WZrCL3y4TIpZfYohfzY_CnvR89lM4pJoXwZmFlTtTdjHN-fQ2xZcNKgU6zd_-jLCssEJSvYtSiIjj-Tgop4nlezkfb8Zt8Lf5EW7b6IVOG2E_UjKA', got 'B41F842082DF20C6354A8B9FF319E00CBE05CE68'
  $.ResultItems[0].MapView[0..3]: expected [33.52692, 45.34303, 34.12277, 45.80953], got [33.3441957500934, 45.35447185451018, 34.3745842499066, 46.073928145489816]

Cluster 3 - fixture geo-places-search-text-max-results-enforced, step task#1, operation SearchText:
  $.ResultItems[0].Title: expected 'STARBUCKS', got 'Starbucks'
  $.ResultItems[0].Address.Label: expected 'STARBUCKS, 500 Mercer St, Seattle, WA 98109-4654, United States', got 'Starbucks, 1912 Pike Pl, Seattle, WA 98101, United States'
  $.ResultItems[0].Address.District: expected 'Lower Queen Anne', got 'Downtown'
  $.ResultItems[0].Address.PostalCode: expected '98109-4654', got '98101'
  $.ResultItems[0].Address.Street / StreetComponents[0].BaseName / .Type / AddressNumber: expected 'Mercer St' / 'Mercer' / 'St' / '500', got 'Pike Pl' / 'Pike' / 'Pl' / '1912'
  $.ResultItems[0].Position: expected [-122.34726, 47.62472], got [-122.340622, 47.6101]
  $.ResultItems[0].Distance: expected 494, got 1327
  $.ResultItems[1].Title/Address/Position/Distance diverge the same way ('442 Terry Ave N, 98109-5210, South Lake Union' vs '102 Pike St, 98101, Downtown'), and $.ResultItems[1].Address.StreetComponents[0].Suffix is missing (expected 'N', got None); $.ResultItems[2] diverges in the same members plus Categories[1].

Emulator code the values come from: state.rs:1956-1968 (synthesize_place_id -> uppercase hex SHA-256, 40 chars), state.rs:310-322 (map_view_around -> symmetric box) with the extent constants at state.rs:3090-3111, and the seeded Seattle Starbucks records at state.rs:3757-3793.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (4 geo-places fixtures, 5 steps).
2. Cause 1 + 2: replay geo-places-geocode-include-place-types-filter#task#1 (POST /v2/geocode) and geo-places-reverse-geocode-political-view-changes-country-code#task#1 (POST /v2/reverse-geocode). Compare $.ResultItems[0].PlaceId against the recorded response: the emulator returns a 40-character uppercase hex string where the service returns a ~106-character base64url token prefixed 'AQAAAD'. Compare $.ResultItems[0].MapView against Position: the emulator's box is exactly symmetric about Position and sized from a PlaceType-keyed constant, and its members carry full double precision instead of the recorded 5 decimal places.
3. Cause 3: replay geo-places-search-text-max-results-enforced#task#1 (POST /v2/search-text) and compare $.ResultItems[*].Title, Address.Label/Street/StreetComponents/District/PostalCode/AddressNumber, Position and Distance: the emulator answers with its own seeded Pike Pl / Pike St / 5th Ave records ('Starbucks', 5-digit postal codes, District 'Downtown', no StreetComponents Suffix) instead of the recorded Mercer St / Terry Ave N places ('STARBUCKS', ZIP+4, real districts, Suffix 'N').
4. Every step reports HTTP 200 in both the recording and the emulator, so the divergence is only visible by comparing the response bodies.
