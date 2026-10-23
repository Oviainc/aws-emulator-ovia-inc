Issue-Source: trace-sweep-divergence
Issue-Trace: geo-places-geocode-include-place-types-filter
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: geo-places-geocode-include-place-types-filter geo-places-geocode-query-components-structured-search geo-places-search-text-max-results-enforced geo-places-reverse-geocode-political-view-changes-country-code
Issue-Shape-Id: com.amazonaws.geoplaces#PlacesService
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].Address.SubRegion.Name
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].MapView[*]
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].PlaceId
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].Position[*]
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.AddressNumber
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.District
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.Label
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.PostalCode
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.Street
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.StreetComponents[*].BaseName
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.StreetComponents[*].Suffix
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.StreetComponents[*].Type
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.SubRegion.Name
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Categories[*]
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Distance
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Position[*]
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Title
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$.ResultItems[*].Address.SubRegion
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$.ResultItems[*].PlaceType
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$.ResultItems[*].Address.Region.Name
Issue-Related: 20261022T192438Z-geo-places-reversegeocode-rejects-valid-politicalview-and-ignores.md
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# geo-places place projection returns wrong sub-region name, language and granularity

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Geocode, ReverseGeocode, SearchText

## Description
All four geo-places divergence clusters in trace set simparity-wire-aws-v1 are HTTP 200 -> HTTP 200 body divergences: the response envelope, status, header set and address-label assembly are right, but the place records the handlers project out of crate::state are projected with the wrong sub-region naming, with no language rendering, and (for ReverseGeocode) at the wrong granularity. 3 causes, on /v2/geocode, /v2/search-text and /v2/reverse-geocode.

Cause 1 of 3: Address.SubRegion.Name is returned with a " County" suffix that AWS does not include.
AWS: Smithy (services/service-com-amazonaws-geoplaces-placesservice/smithy-model/com-amazonaws-geoplaces-placesservice.json) models Address.SubRegion as SubRegion{Code (len 0..3), Name (len 0..200)}, documented "Name for the county or sub-region." The model does not state the naming convention, so: recorded behavior only -- across three independent fixtures and two operations (Geocode and SearchText), every Seattle-area result carries Address.SubRegion.Name = "King", i.e. the bare county name, never "King County". The same recordings show the suffix is likewise absent from Address.Label (expected label "STARBUCKS, 500 Mercer St, Seattle, WA 98109-4654, United States"), so AWS keeps the county's bare name in the SubRegion component.
Emulator: returns Address.SubRegion.Name = "King County" on every result whose address sits in that sub-region, for both Geocode and SearchText. The seeded sub-region name carries the administrative suffix as part of the name itself, so every result projected from it repeats it.
Why: SubRegion.Name is the field a client groups, filters or displays results by county with. A client matching the AWS value ("King", "San Francisco") against the emulator's ("King County", "San Francisco County") gets no match, so county-keyed joins, de-duplication and UI labels silently come up empty against the emulator while working against AWS; the difference is also inconsistent in-process, since the emulator reports the suffixed string in a field documented as the county NAME.
Why not the other way round: the affected value is a response data field, not a generated id or a message, and it is stable across every fixture, so it is a fixed convention rather than environment-specific data.
Fix: the seeded US sub-region names in the service's state (crate::state's seeded catalog, the us_area() sub-region argument and the SubRegion-typed place records built from it) -- hold the bare county name on AddressRecord.sub_region_name and apply any administrative suffix only where a Title or Label needs it, not in the SubRegion component every operation projects.
Traces: geo-places-geocode-include-place-types-filter#task#1 (also geo-places-geocode-query-components-structured-search#task#1 in the same cluster) and geo-places-search-text-max-results-enforced#task#1.

Cause 2 of 3: result text is never rendered in the result's language -- non-English places come back in English.
AWS: Smithy models ReverseGeocodeResultItem.Title as "The localized display name of this result item based on request parameter language", and ReverseGeocodeRequest.Language as LanguageTag (len 2..35), "a list of BCP 47 compliant language codes for the results to be rendered in. If there is no data for the result in the requested language, data will be returned in the default language for the entry". The recording shows exactly that: for a position in a disputed territory the default (Ukrainian-attributed) result carries Title/Address.Label "Первомайський район, Україна" with Address.Country.Name "Україна", and the same point under the RU political view carries "Первомайский район, Южный федеральный округ, Россия" with Country.Name "Россия" and Region.Name "Южный федеральный округ" -- the display text follows the entry's own language, not a fixed one.
Emulator: returns Title/Address.Label "Crimea, Ukraine" and "Crimea, Russia", Country.Name "Ukraine"/"Russia", Region.Name "Crimea" -- English text in every case. The handler validates Language (crate::state::check_language is called from ops/reverse_geocode.rs) and then never uses it: result text is produced by place.title_modeled_in(political_view) / place.address_modeled_in(political_view), which take the political view only, and the seeded AddressRecord stores an entry's language tag ("uk", "ru", "fr") while holding its names in English. The emulator therefore reports StreetComponents.Language = the entry's non-English tag alongside English name text, which is self-contradictory.
Why: a client asking for localized places (or simply reading a non-English entry) gets English display names, so any UI that renders Title/Address.Label/Country.Name shows the wrong script and the wrong words for every non-English place, and the per-entry Language it is handed does not describe the text it got. Language is accepted rather than refused, so the client has no signal that the parameter did nothing.
Fix: the ReverseGeocode (and the sibling Geocode/SearchText) result assembly plus the seeded catalog in crate::state -- carry per-language name text on the place/address records and select it from the request's Language, falling back to the entry's default language; thread the request's Language into title_modeled_in/address_modeled_in instead of dropping it after validation.
Traces: geo-places-reverse-geocode-political-view-changes-country-code#task#1 (default view, Ukrainian text expected) and #task#2 (PoliticalView RU, Russian text expected).

Cause 3 of 3: ReverseGeocode resolves a position to a Region instead of the finest place covering it, so the address omits SubRegion/Locality and Distance is non-zero for a point inside the result.
AWS: Smithy models ReverseGeocodeRequest.QueryPosition as @required Position [longitude, latitude] with MaxResults "Default value: 1", and ReverseGeocodeResultItem.Distance as "The distance in meters from the QueryPosition" (default 0). The recording for QueryPosition [33.85939, 45.7142] returns one item with PlaceType "Locality", Position equal to the queried point, Distance 0, and a full administrative address for that point (Address.SubRegion.Name set, Address.Locality set). AWS answers with the finest-granularity place that contains the position.
Emulator: returns PlaceType "Region" for the same point -- the seeded region-level record (Position [34.1, 45.3], REGION_EXTENT_M) -- with Distance 21717, Address.SubRegion absent and Address.Locality absent. crate::state::at_position() selects every record whose MapView covers the point, and the catalog holds only a Region-granularity record over that area (its locality-granularity records sit elsewhere), so the single returned item is a region whose centroid is ~22 km from the request and whose AddressRecord has empty locality/sub_region fields.
Why: reverse geocoding exists to turn a coordinate into an address. A client gets a region name where AWS gives the locality, no Locality/SubRegion component to render or store at all, and a Distance that contradicts the request (non-zero for a point that is inside the returned place), so address-from-position features (delivery, check-in, geofence labelling) degrade to country/region granularity with no indication that a finer answer existed.
Fix: ReverseGeocode's position resolution and the seeded catalog -- rank covering candidates finest-granularity-first (and seed locality/sub-region-level coverage for the areas the corpus queries) so the default MaxResults 1 yields the finest place containing the point, with its Address.SubRegion/Locality populated and Distance measured from QueryPosition to that place.
Traces: geo-places-reverse-geocode-political-view-changes-country-code#task#1 and #task#2.

Relationship to the open issue "geo-places ReverseGeocode rejects valid PoliticalView and ignores QueryPosition" (20261022T192438Z-geo-places-reversegeocode-rejects-valid-politicalview-and-ignores.md): that issue's recorded divergences were status 200->400 on PoliticalView and a result ~2,400 km away (Paris) for the queried position. Both of those are gone from this replay -- task#2 now answers 200 and the result is now a place over the queried territory -- and the replay reports no owning issue for any of the clusters above, so these are the residual, differently-keyed divergences on the same fixture and are filed here rather than assumed covered.

Not claimed as defects here (not filed, consistent with the carve-out in that open issue): the exact PlaceId values (opaque generated ids -- AWS returns a long opaque token, the emulator a 40-hex digest, both inside the modeled len 0..500); coordinate precision of Position (differences of ~60 m); MapView extents; and WHICH reference-data entries the catalog holds (e.g. the recorded Starbucks at 500 Mercer St / 442 Terry Ave N versus the emulator's Pike Pl / Pike St entries, their PostalCode/District/Street/AddressNumber/Distance/Categories, and the recorded "STARBUCKS" brand-name casing). Those are real-world map-dataset content, not wire contract; the SearchText cluster's structure (item count under MaxResults, label assembly order, component split) matches the recording.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 5 steps total, 0 passed, 5 mismatched, 4 divergence clusters, all service "geo-places", all expected_status 200 / actual_status 200, none carrying stub_path or owned_by.

Cluster 1 -- fixture geo-places-geocode-include-place-types-filter, step task#1 (Geocode; also affects geo-places-geocode-query-components-structured-search#task#1):
  $.ResultItems[0].Address.SubRegion.Name: expected "King", got "King County"
  (not claimed: PlaceId, Position[0] -122.32945 vs -122.330062, Position[1] 47.60357 vs 47.603832, MapView[0..3])

Cluster 4 -- fixture geo-places-search-text-max-results-enforced, step task#1 (SearchText):
  $.ResultItems[0].Address.SubRegion.Name: expected "King", got "King County"
  $.ResultItems[1].Address.SubRegion.Name: expected "King", got "King County"
  $.ResultItems[2].Address.SubRegion.Name: expected "King", got "King County"
  (not claimed: the differing POI entries themselves -- Title casing "STARBUCKS" vs "Starbucks", Label/Street/AddressNumber/PostalCode/District/StreetComponents/Position/Distance/Categories of each item)

Cluster 2 -- fixture geo-places-reverse-geocode-political-view-changes-country-code, step task#1 (ReverseGeocode, default political view):
  $.ResultItems[0].PlaceType: expected "Locality", got "Region"
  $.ResultItems[0].Title: expected "Первомайський район, Україна", got "Crimea, Ukraine"
  $.ResultItems[0].Address.Label: expected "Первомайський район, Україна", got "Crimea, Ukraine"
  $.ResultItems[0].Address.Country.Name: expected "Україна", got "Ukraine"
  $.ResultItems[0].Address.SubRegion: expected {"Name": "Автономна Республіка Крим"}, got absent
  $.ResultItems[0].Address.Locality: expected "Первомайський район", got absent
  $.ResultItems[0].Distance: expected 0, got 21717
  (not claimed: PlaceId, Position [33.85939, 45.7142] vs [34.1, 45.3] -- the catalog centroid, MapView extents)

Cluster 3 -- same fixture, step task#2 (ReverseGeocode, PoliticalView RU):
  $.ResultItems[0].PlaceType: expected "Locality", got "Region"
  $.ResultItems[0].Title / Address.Label: expected "Первомайский район, Южный федеральный округ, Россия", got "Crimea, Russia"
  $.ResultItems[0].Address.Country.Name: expected "Россия", got "Russia"
  $.ResultItems[0].Address.Region.Name: expected "Южный федеральный округ", got "Crimea"
  $.ResultItems[0].Address.SubRegion: expected {"Name": "Республика Крым"}, got absent
  $.ResultItems[0].Address.Locality: expected "Первомайский район", got absent
  $.ResultItems[0].Distance: expected 0, got 21717

Code the divergences trace back to in this checkout:
  services/service-com-amazonaws-geoplaces-placesservice/src/ops/reverse_geocode.rs:206-215 -- Language is validated via crate::state::check_language(input.language.as_deref()) and then never read again.
  same file, lines 266-292 -- result items are built with place.title_modeled_in(view) / place.address_modeled_in(view), where view is the political view only (no language argument exists).
  services/service-com-amazonaws-geoplaces-placesservice/src/state.rs:1051-1064 -- title_modeled_in / address_modeled_in take Option<&str> political_view and nothing else.
  state.rs:2131-2141 -- at_position() filters by place.covers(position) (MapView containment) with no finest-granularity ranking.
  state.rs:3074-3083 -- the only record covering the queried territory is PlaceRecord::new("Crimea", PLACE_TYPE_REGION, [34.1, 45.3], crimea_ukraine, REGION_EXTENT_M), whose AddressRecord (state.rs:2383-2412) has empty locality and sub_region_name.
  state.rs:2355 -- let seattle = us_area("WA", "Washington", "King County", "Seattle", "98104"); the sub-region name is seeded with the " County" suffix, and state.rs:2662 titles the SubRegion place the same way.

Smithy (services/service-com-amazonaws-geoplaces-placesservice/smithy-model/com-amazonaws-geoplaces-placesservice.json): Geocode POST /v2/geocode, ReverseGeocode POST /v2/reverse-geocode, SearchText POST /v2/search-text, all success 200; ReverseGeocodeRequest.QueryPosition @required Position len 2..2; Language = LanguageTag len 2..35; ReverseGeocodeResultItem.PlaceType @required enum incl. Locality/Region; .Distance DistanceMeters default 0 "distance in meters from the QueryPosition"; .Title "localized display name of this result item based on request parameter language"; Address.SubRegion = SubRegion{Code, Name "Name for the county or sub-region"}; errors AccessDenied(403)/Internal(500)/Throttling(429)/Validation(400) -- none of which applies, every step returned 200.

## Steps to Reproduce
1. Clone this repository, boot the emulator built from the checkout, and replay the wire corpus with replay_wire_corpus() (trace set simparity-wire-aws-v1). Summary reports 5/5 steps mismatched, 4 divergence clusters, all service geo-places, bootstrap_signal false, state_reset_failures 0.
2. Cause 1 (sub-region naming): fixture geo-places-geocode-include-place-types-filter, step task#1 -- POST /v2/geocode for the Seattle query. Read $.ResultItems[0].Address.SubRegion.Name: the emulator answers "King County" where the recording has "King". Repeat with fixture geo-places-search-text-max-results-enforced, step task#1 (POST /v2/search-text) and see the same suffix on all three result items, and with geo-places-geocode-query-components-structured-search#task#1.
3. Cause 2 (language rendering): fixture geo-places-reverse-geocode-political-view-changes-country-code, step task#1 -- POST /v2/reverse-geocode with QueryPosition [33.85939, 45.7142]. Read $.ResultItems[0].Title, Address.Label and Address.Country.Name: the emulator returns English text ("Crimea, Ukraine" / "Ukraine") where the recording returns the entry's own language ("Первомайський район, Україна" / "Україна"). Step task#2 of the same fixture adds PoliticalView RU: the recording switches the text to Russian ("Первомайский район, Южный федеральный округ, Россия"), the emulator stays in English ("Crimea, Russia").
4. Cause 3 (granularity): in the same two steps read $.ResultItems[0].PlaceType, Address.SubRegion, Address.Locality and Distance: the emulator answers PlaceType "Region" with no SubRegion and no Locality and Distance 21717, where the recording answers PlaceType "Locality" with both components populated and Distance 0 for a point inside the result.
5. After a fix, re-replay the fixtures named in Issue-Trace/Issue-Step and Issue-Trace-Ids -- geo-places-geocode-include-place-types-filter#task#1, geo-places-search-text-max-results-enforced#task#1, and geo-places-reverse-geocode-political-view-changes-country-code#task#1 and #task#2 -- all four clusters must clear, not just the representative.
