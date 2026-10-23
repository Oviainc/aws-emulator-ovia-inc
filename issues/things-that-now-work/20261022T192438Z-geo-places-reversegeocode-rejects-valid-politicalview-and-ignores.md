Issue-Source: trace-sweep-divergence
Issue-Trace: geo-places-reverse-geocode-political-view-changes-country-code
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: geo-places-reverse-geocode-political-view-changes-country-code
Issue-Shape-Id: com.amazonaws.geoplaces#PlacesService
Issue-Key: geo-places|ReverseGeocode|2xx->4xx|$.ResultItems
Issue-Key: geo-places|ReverseGeocode|2xx->4xx|$headers.x-amz-geo-pricing-bucket
Issue-Key: geo-places|ReverseGeocode|2xx->4xx|status
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$.ResultItems[*].Address.Country.Code2
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$.ResultItems[*].Address.Country.Code3
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$.ResultItems[*].Address.Country.Name
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$.ResultItems[*].Address.Label
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$.ResultItems[*].Address.Locality
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$.ResultItems[*].Address.SubRegion.Name
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$.ResultItems[*].Distance
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$.ResultItems[*].MapView[*]
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$.ResultItems[*].PlaceId
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$.ResultItems[*].Position[*]
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$.ResultItems[*].Title
Issue-Key: geo-places|ReverseGeocode|2xx->2xx|$headers.x-amz-geo-pricing-bucket
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# geo-places ReverseGeocode rejects valid PoliticalView and ignores QueryPosition

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReverseGeocode

## Description
The emulator's ReverseGeocode implementation does not honour two input members of ReverseGeocodeRequest: it fails outright when PoliticalView is supplied, and it does not resolve the result from QueryPosition. 2 causes, both on /v2/reverse-geocode.

Cause 1 of 2: a syntactically valid PoliticalView is rejected with HTTP 400.
AWS: Smithy (services/service-com-amazonaws-geoplaces-placesservice/smithy-model/com-amazonaws-geoplaces-placesservice.json) models ReverseGeocodeRequest.PoliticalView as CountryCode, @length(min:2,max:3), @pattern ^([A-Z]{2}|[A-Z]{3})$ -- so an alpha-3 code is in-range and must be accepted. Its documentation: "The alpha-2 or alpha-3 character code for the political view of a country. The political view applies to the results of the request to represent unresolved territorial claims through the point of view of the specified country." The operation's only modelled client-input error is ValidationException (HTTP 400), which does not apply to a value satisfying the member's constraints. The recorded response is HTTP 200 whose ResultItems[0] carries the political-view-adjusted place (Country.Code2 RU / Code3 RUS, localized names) plus the echoed GeocodeResultItem.PoliticalView = "RUS" (output member CountryCode3, @pattern ^[A-Z]{3}$).
Emulator: answers HTTP 400 with no ResultItems and no x-amz-geo-pricing-bucket header at all. The replay record does not capture the returned error code, so the required change is the status (200) and the body shape, not a particular message.
Why: any client that sets PoliticalView -- the whole purpose of the parameter, for territorial-dispute-aware rendering -- gets a hard client error instead of a result set, so the call cannot be made at all and SDK retries will not help (400 is non-retryable).
Fix: the ReverseGeocode handler's input validation must accept 2- and 3-character PoliticalView values per the shape's pattern/length and pass them through; the result assembly must then set GeocodeResultItem.PoliticalView on each returned item.
Traces: geo-places-reverse-geocode-political-view-changes-country-code#task#2 (expected 200, got 400).

Cause 2 of 2: the returned place is not the place at QueryPosition.
AWS: Smithy ReverseGeocodeRequest.QueryPosition is @required Position [longitude, latitude], with optional QueryRadius (DistanceMeters, range 1..21000000); ReverseGeocodeResultItem.Distance is documented as "The distance in meters from the QueryPosition". AWS therefore returns the place(s) containing/nearest the queried point -- the recording for QueryPosition [33.85939, 45.7142] returns a Locality whose Position equals the queried point, Distance 0, and the locality/country of that point.
Emulator: returns a completely different place -- Title "Paris", Address.Label "Paris, Ile-de-France, France", Country.Code2 "FR"/Code3 "FRA", Position [2.352222, 48.856613] -- and reports Distance 2401267, i.e. it hands back an entry roughly 2,400 km from the requested position. The emulator is evidently picking an entry from a small built-in catalogue with no containment or radius check (Hypothesis: nearest-of-catalogue with no coverage/QueryRadius bound).
Why: reverse geocoding is positional by definition. A client converting coordinates to an address gets an address on another continent, and the self-reported Distance contradicts the request, so address-from-position features (delivery, check-in, geofence labelling) silently produce wrong data rather than an empty ResultItems list.
Fix: ReverseGeocode's position resolution/state lookup -- resolve against QueryPosition, bound candidates by QueryRadius (and return an empty ResultItems list when nothing is in range) and compute Distance from QueryPosition.
Traces: geo-places-reverse-geocode-political-view-changes-country-code#task#1.

Not claimed as defects here: the exact PlaceId values (opaque generated ids, environment-specific) and small differences in map reference data (coordinate precision, MapView extents, "King County" style sub-region naming). The x-amz-geo-pricing-bucket header value also diverges on this fixture; that defect is shared with Geocode/SearchText and is filed in the companion geo-places response-composition issue rather than duplicated here.

## Evidence
Fixture geo-places-reverse-geocode-political-view-changes-country-code.

Step task#2 (PoliticalView supplied): expected status 200 with ResultItems[0] = {PlaceType: Locality, Address.Country.Code2 "RU", Code3 "RUS", Position [33.85939, 45.7142], Distance 0, PoliticalView "RUS"} and header x-amz-geo-pricing-bucket present. Actual: status 400, $.ResultItems missing, x-amz-geo-pricing-bucket missing.

Step task#1 (same QueryPosition, no political view): expected ResultItems[0].Address.Country.Code2 "UA" / Code3 "UKR", Locality of the queried point, Position [33.85939, 45.7142], Distance 0. Actual: Title "Paris", Address.Label "Paris, Ile-de-France, France", Country.Code2 "FR" / Code3 "FRA", Position [2.352222, 48.856613], Distance 2401267.

Smithy: ReverseGeocodeRequest.PoliticalView = CountryCode @length(2,3) @pattern ^([A-Z]{2}|[A-Z]{3})$; QueryPosition @required; QueryRadius range 1..21000000; ReverseGeocodeResponse.PricingBucket @required httpHeader x-amz-geo-pricing-bucket; errors = AccessDenied(403)/Internal(500)/Throttling(429)/Validation(400).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), trace set simparity-wire-aws-v1.
2. Fixture geo-places-reverse-geocode-political-view-changes-country-code, step task#1: POST /v2/reverse-geocode with QueryPosition [33.85939, 45.7142]. Observe the response describes Paris, FR with Distance 2401267 instead of the locality at that position with Distance 0.
3. Same fixture, step task#2: POST /v2/reverse-geocode with the same QueryPosition plus PoliticalView "RUS". Observe HTTP 400 instead of HTTP 200 with ResultItems[0].PoliticalView "RUS" and the RU-view country fields.
