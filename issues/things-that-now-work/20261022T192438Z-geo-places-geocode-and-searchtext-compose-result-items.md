Issue-Source: trace-sweep-divergence
Issue-Trace: geo-places-geocode-include-place-types-filter
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: geo-places-geocode-include-place-types-filter geo-places-geocode-query-components-structured-search geo-places-search-text-max-results-enforced
Issue-Shape-Id: com.amazonaws.geoplaces#PlacesService
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].Address.PostalCode
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].Address.SubRegion.Name
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].MapView[*]
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].MatchScores.Components
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].ParsedQuery
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].PlaceId
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].Position[*]
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].Title
Issue-Key: geo-places|Geocode|2xx->2xx|$headers.x-amz-geo-pricing-bucket
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*]
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].Address.Label
Issue-Key: geo-places|Geocode|2xx->2xx|$.ResultItems[*].PlaceType
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.AddressNumber
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.District
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.Label
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.PostalCode
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.Street
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.StreetComponents
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Address.SubRegion.Name
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].BusinessChains
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].BusinessChains[*].Id
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Categories
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Categories[*]
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Distance
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].PlaceType
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Position[*]
Issue-Key: geo-places|SearchText|2xx->2xx|$.ResultItems[*].Title
Issue-Key: geo-places|SearchText|2xx->2xx|$headers.x-amz-geo-pricing-bucket
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# geo-places Geocode and SearchText compose result items from the wrong query inputs

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Geocode

## Description
The emulator's Places search path answers 200 on /v2/geocode and /v2/search-text, but builds each result item from a built-in catalogue lookup that ignores structured query inputs and omits several modelled result members. 4 causes.

Cause 1 of 4: Geocode locality results are not assembled -- no full address label, no postal code, and MatchScores/ParsedQuery dropped.
AWS: Smithy GeocodeResultItem.Title is "The localized display name of this result item" and Address.Label is "Assembled address value built out of the address components, according to the regional postal rules. This is the correctly formatted address." GeocodeResultItem also models MatchScores (MatchScoreDetails, "Indicates how well the entire input matches the returned") and ParsedQuery (GeocodeParsedQuery) as plain output members -- they are NOT gated behind AdditionalFeatures, whose enum is only TimeZone|Access|SecondaryAddresses|Intersections. The recording shows a Locality result with Title and Address.Label both "Seattle, WA, United States", a populated Address.PostalCode, MatchScores.Components {Address:{Locality:1}} and ParsedQuery {Address:{Locality:[{StartIndex,EndIndex,Value,QueryComponent}]}}.
Emulator: Title is the bare component "Seattle" (Address.Label likewise unassembled for this item), Address.PostalCode is the empty string, and MatchScores.Components and ParsedQuery are absent (null).
Why: clients display Address.Label/Title as the single formatted address line and get a bare token instead; postal code is empty where AWS always supplies one for a locality; and relevance/parse consumers (ranking on MatchScores, highlighting the parsed query span) see no data at all, which reads as "no match information available" rather than a perfect match.
Fix: Geocode's result-item assembly/serializer -- build Address.Label and Title from the address components per regional rules, populate Address.PostalCode, and emit MatchScores and ParsedQuery for every geocode result.
Traces: geo-places-geocode-include-place-types-filter#task#1.

Cause 2 of 4: Geocode ignores QueryComponents (structured search).
AWS: Smithy GeocodeRequest accepts either QueryText or QueryComponents (GeocodeQueryComponents) and MaxResults (range 1..100). For a structured query of Country/Region/Locality the recording returns exactly ONE result -- the matching Locality (PlaceType "Locality", Title/Label "Seattle, WA, United States") -- with ParsedQuery echoing each supplied component (QueryComponent "Country"/"Region"/"Locality") and MatchScores.Components {Address:{Country:1,Region:1,Locality:1}}.
Emulator: returns an unrelated point address as ResultItems[0] (PlaceType "PointAddress", Title "400 Broad Street", Label "400 Broad St, Seattle, WA 98109, United States") plus four EXTRA items (ResultItems[1..4], e.g. "Pike Place Market" PointOfInterest) that the recorded response does not contain, and again omits MatchScores.Components and ParsedQuery. The structured components are evidently flattened into a free-text catalogue search (Hypothesis: QueryComponents is concatenated or discarded before lookup).
Why: structured geocoding is used precisely to avoid free-text ambiguity. A client that submits components gets a point address of unrelated granularity at index 0 and a longer result list than AWS returns, so "take the first result" logic resolves a city to a street address, and no ParsedQuery is available to detect the mismatch.
Fix: Geocode's handler -- resolve against QueryComponents (honouring component granularity so a Country/Region/Locality query yields the Locality) and cap/shape the result list as the structured match implies, then emit ParsedQuery/MatchScores keyed by the supplied components.
Traces: geo-places-geocode-query-components-structured-search#task#1.

Cause 3 of 4: SearchText returns catalogue neighbours instead of QueryText matches, and drops Address.StreetComponents.
AWS: Smithy SearchTextRequest has QueryText (@length 1..200), BiasPosition, MaxResults (1..100); SearchTextResultItem.Address is the Address shape, which models StreetComponents (StreetComponentsList, "Components of the street"), and Distance is the distance from the bias position. The recording for a branded text query returns the brand's locations: every item Title "STARBUCKS" with the full assembled label (e.g. "STARBUCKS, 500 Mercer St, Seattle, WA 98109-4654, United States"), Address.StreetComponents present ([{BaseName,Type,TypePlacement,TypeSeparator,Language}]), BusinessChains[0].Id "Starbucks", and Distance consistent with the bias position.
Emulator: returns different, non-matching places in each slot -- ResultItems[0] "Starbucks Reserve Roastery" at 1124 Pike St, ResultItems[1] "Space Needle" (a different brand entirely, with BusinessChains and Categories differing), ResultItems[2] of a different PlaceType -- omits Address.StreetComponents on every item, emits an extra Categories entry, lower-cases BusinessChains[0].Id to "starbucks", and reports Distance values unrelated to the recorded ones (1799 vs 494).
Why: a text search for a brand returns unrelated POIs, so result lists, chain filtering on BusinessChains.Id (case-sensitive string compare) and any street-component parsing all break; clients cannot rely on the result set reflecting QueryText at all.
Fix: SearchText's handler and result-item assembly -- rank/filter candidates by QueryText (brand/BusinessChains match before proximity), populate Address.StreetComponents, and preserve BusinessChains.Id casing as stored rather than normalising it.
Traces: geo-places-search-text-max-results-enforced#task#1.

Cause 4 of 4: the required x-amz-geo-pricing-bucket response header carries the operation name instead of a pricing tier.
AWS: Smithy marks PricingBucket @required on GeocodeResponse, ReverseGeocodeResponse and SearchTextResponse, bound to httpHeader x-amz-geo-pricing-bucket. Recorded behavior only (the model gives no value list): every recorded Places response -- Geocode, ReverseGeocode and SearchText alike -- returns the same pricing-tier token "Core".
Emulator: echoes the operation name in the header -- "Geocode" on /v2/geocode, "SearchText" on /v2/search-text, "ReverseGeocode" on /v2/reverse-geocode -- i.e. a per-operation value where AWS returns a tier.
Why: the header exists so clients and cost tooling can attribute a request to a billing tier; an operation name is not a tier, so any consumer bucketing requests by it mis-attributes every call and never sees a tier change.
Fix: the Places response serializer / shared pricing-bucket computation -- set PricingBucket from the request's pricing tier, not from the operation name.
Traces: geo-places-geocode-include-place-types-filter#task#1, geo-places-geocode-query-components-structured-search#task#1, geo-places-search-text-max-results-enforced#task#1 (the same header defect also appears on geo-places-reverse-geocode-political-view-changes-country-code#task#1, filed in the companion ReverseGeocode issue only as context -- fix it once in the shared serializer).

Not claimed as defects: the exact PlaceId values (opaque generated ids, environment-specific) and pure reference-data differences such as coordinate precision, MapView extents and sub-region naming style ("King County" vs "King").

## Evidence
Fixture geo-places-geocode-include-place-types-filter#task#1 (POST /v2/geocode, expected 200, got 200):
  $.ResultItems[0].Title: expected 'Seattle, WA, United States', got 'Seattle'
  $.ResultItems[0].Address.PostalCode: expected '98104', got ''
  $.ResultItems[0].MatchScores.Components: expected {'Address': {'Locality': 1}}, got None
  $.ResultItems[0].ParsedQuery: expected {'Address': {'Locality': [{'StartIndex': 0, 'EndIndex': 7, 'Value': 'Seattle', 'QueryComponent': 'Query'}]}}, got None
  $headers.x-amz-geo-pricing-bucket: expected 'Core', got 'Geocode'

Fixture geo-places-geocode-query-components-structured-search#task#1 (POST /v2/geocode, expected 200, got 200):
  $.ResultItems[0].PlaceType: expected 'Locality', got 'PointAddress'
  $.ResultItems[0].Title: expected 'Seattle, WA, United States', got '400 Broad Street'
  $.ResultItems[0].Address.Label: expected 'Seattle, WA, United States', got '400 Broad St, Seattle, WA 98109, United States'
  $.ResultItems[0].MatchScores.Components: expected {'Address': {'Country': 1, 'Region': 1, 'Locality': 1}}, got None
  $.ResultItems[0].ParsedQuery: expected the three parsed components (Country 'USA', Region 'Washington', Locality 'Seattle'), got None
  $.ResultItems[1]..$.ResultItems[4]: extra (expected None, got e.g. {'PlaceType': 'PointOfInterest', 'Title': 'Pike Place Market', ...})

Fixture geo-places-search-text-max-results-enforced#task#1 (POST /v2/search-text, expected 200, got 200):
  $.ResultItems[0].Title: expected 'STARBUCKS', got 'Starbucks Reserve Roastery'
  $.ResultItems[0].Address.Label: expected 'STARBUCKS, 500 Mercer St, Seattle, WA 98109-4654, United States', got '1124 Pike St, Seattle, WA 98101, United States'
  $.ResultItems[0].Address.StreetComponents: expected [{'BaseName': 'Mercer', 'Type': 'St', 'TypePlacement': 'AfterBaseName', 'TypeSeparator': ' ', 'Language': 'en'}], got None
  $.ResultItems[0].BusinessChains[0].Id: expected 'Starbucks', got 'starbucks'
  $.ResultItems[0].Categories[1]: extra (got {'Id': 'restaurant', ...})
  $.ResultItems[1].Title: expected 'STARBUCKS', got 'Space Needle'
  $.ResultItems[2].PlaceType / Address.* / Categories / BusinessChains also diverge
  $headers.x-amz-geo-pricing-bucket: expected 'Core', got 'SearchText'

Smithy (services/service-com-amazonaws-geoplaces-placesservice/smithy-model/com-amazonaws-geoplaces-placesservice.json): GeocodeResultItem.MatchScores/ParsedQuery are ungated output members; GeocodeAdditionalFeature = [TimeZone, Access, SecondaryAddresses, Intersections]; Address.Label = assembled formatted address; Address.StreetComponents = StreetComponentsList; GeocodeResponse/SearchTextResponse.PricingBucket @required httpHeader x-amz-geo-pricing-bucket.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), trace set simparity-wire-aws-v1.
2. Fixture geo-places-geocode-include-place-types-filter, step task#1: POST /v2/geocode for a locality. Observe Title 'Seattle' instead of the assembled label, empty Address.PostalCode, and null MatchScores/ParsedQuery.
3. Fixture geo-places-geocode-query-components-structured-search, step task#1: POST /v2/geocode with QueryComponents {Country, Region, Locality}. Observe a PointAddress first item plus four extra result items, and null ParsedQuery/MatchScores.
4. Fixture geo-places-search-text-max-results-enforced, step task#1: POST /v2/search-text with the branded QueryText and a bias position. Observe non-matching POIs, missing Address.StreetComponents, and lower-cased BusinessChains[0].Id.
5. On each of the responses above, inspect the x-amz-geo-pricing-bucket response header: it carries the operation name rather than the 'Core' pricing tier.
