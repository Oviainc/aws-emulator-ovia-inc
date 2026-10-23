Issue-Source: trace-sweep-divergence
Issue-Trace: geo-routes-calculate-routes-truck-travel-mode-options-wiring-success
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: geo-routes-calculate-routes-truck-travel-mode-options-wiring-success geo-routes-calculate-routes-zero-max-alternatives-returns-single-route
Issue-Shape-Id: com.amazonaws.georoutes#RoutesService
Issue-Key: geo-routes|CalculateRoutes|2xx->2xx|$.Routes[*].Legs[*].VehicleLegDetails.Arrival.Place.OriginalPosition[*]
Issue-Key: geo-routes|CalculateRoutes|2xx->2xx|$.Routes[*].Legs[*].VehicleLegDetails.Arrival.Place.Position[*]
Issue-Key: geo-routes|CalculateRoutes|2xx->2xx|$.Routes[*].Legs[*].VehicleLegDetails.Departure.Place.OriginalPosition[*]
Issue-Key: geo-routes|CalculateRoutes|2xx->2xx|$.Routes[*].Legs[*].VehicleLegDetails.Departure.Place.Position[*]
Issue-Key: geo-routes|CalculateRoutes|2xx->2xx|$.Routes[*].Legs[*].VehicleLegDetails.Summary
Issue-Key: geo-routes|CalculateRoutes|2xx->2xx|$.Routes[*].Legs[*].VehicleLegDetails.TravelSteps[*]
Issue-Key: geo-routes|CalculateRoutes|2xx->2xx|$.Routes[*].MajorRoadLabels[*]
Issue-Key: geo-routes|CalculateRoutes|2xx->2xx|$headers.x-amz-geo-pricing-bucket
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# CalculateRoutes response synthesis ignores request and drops modeled leg detail

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CalculateRoutes

## Description
Replay of the geo-routes wire corpus shows CalculateRoutes answering HTTP 200 with a response body/headers that are synthesized without regard to the request or to the modeled response shape. Status matches (200 -> 200); the body and one response header diverge. This issue bundles FIVE distinct causes, all in the CalculateRoutes response path of the geo-routes service. A fixer must close all five before submitting.

Cause 1 of 5 - RouteVehiclePlace positions are quantized to 4 decimal places, and Position echoes OriginalPosition.
AWS: Smithy: RouteVehiclePlace.OriginalPosition and .Position both target Position23 (list of Double, @length min 2 max 3). OriginalPosition is documented as "Position provided in the request", so it must be echoed back at full double precision, un-rounded. Position is documented as "Position in World Geodetic System (WGS 84) format: [longitude, latitude]" and in the recording is the map-matched/road-snapped point, which is a DIFFERENT value from OriginalPosition (recorded behavior only for the snapping itself).
Emulator: every coordinate comes back rounded to 4 decimals, and Position is returned identical to OriginalPosition. Arrival.Place.OriginalPosition expected [-122.3088001, 47.4501999], got [-122.3088, 47.4502]; Arrival.Place.Position expected [-122.3029601, 47.45124], got [-122.3088, 47.4502] (i.e. the rounded input); Departure.Place.OriginalPosition[0] expected -122.3410001, got -122.341; Departure.Place.Position expected [-122.3409556, 47.6089359], got [-122.341, 47.6089].
Why: a client that round-trips its own request coordinates (or diffs OriginalPosition against Position to detect how far a waypoint was snapped to the road network) gets back a silently de-precisioned value and a zero-length snap. Anything that keys, caches or asserts on the echoed request position will not match what it sent.
Fix: the CalculateRoutes response builder/serializer in the geo-routes crate - stop rounding Position23 values (serialize the Double as given) and carry the request position through to OriginalPosition verbatim while deriving Position separately.
Traces: geo-routes-calculate-routes-truck-travel-mode-options-wiring-success#task#1, geo-routes-calculate-routes-zero-max-alternatives-returns-single-route#task#1.

Cause 2 of 5 - VehicleLegDetails.Summary is never emitted.
AWS: Smithy: RouteVehicleLegDetails.Summary targets RouteVehicleSummary {Overview: RouteVehicleOverviewSummary, TravelOnly: RouteVehicleTravelOnlySummary}; RouteVehicleOverviewSummary requires Distance (meters) and Duration (seconds) and also carries BestCaseDuration/TypicalDuration, RouteVehicleTravelOnlySummary requires Duration. The recording shows AWS populating it: Summary = {Overview: {BestCaseDuration, Distance, Duration}, TravelOnly: {BestCaseDuration, Duration}} (recorded example values 1344 s / 23113 m).
Emulator: the member is absent from the leg entirely - "$.Routes[0].Legs[0].VehicleLegDetails.Summary: missing ... got None".
Why: leg distance and duration are the primary result of a routing call; a client reading leg.VehicleLegDetails.Summary.Overview.Distance gets a null/absent optional and either crashes on unwrap or reports a route with no length or ETA.
Fix: the CalculateRoutes leg-building code in the geo-routes crate - populate RouteVehicleSummary (Overview + TravelOnly) from the computed leg, consistent with the leg's own geometry and travel steps.
Traces: geo-routes-calculate-routes-truck-travel-mode-options-wiring-success#task#1, geo-routes-calculate-routes-zero-max-alternatives-returns-single-route#task#1.

Cause 3 of 5 - travel-step detail members are emitted unconditionally and ExitNumber is dropped.
AWS: Smithy: RouteVehicleTravelStep requires only Duration and Type; CurrentRoad, NextRoad, Instruction, ExitNumber and the per-type *StepDetails members are optional and request-gated - Instruction is documented "Only available when the TravelStepType is Default" (RouteTravelStepType = Default | TurnByTurn), and road/span detail is gated on LegAdditionalFeatures / SpanAdditionalFeatures. The recorded Depart step is the minimal form: {Distance, Duration, ExitNumber: [], GeometryOffset: 0, Type: "Depart"} - no Instruction, no CurrentRoad, no NextRoad.
Emulator: the Depart step carries Instruction ("Depart on <generated road> and continue for 1.5 km."), CurrentRoad and NextRoad (each a RouteRoad with generated RoadName) even though the request did not ask for that detail level, and it omits ExitNumber which the recording returns as an empty list. The generated road names and the step's Distance/Duration magnitudes are data-dependent and are quoted only as examples; the defect is that detail membership does not follow the request and that ExitNumber is missing.
Why: a client that switches on the presence of Instruction (or of CurrentRoad/NextRoad) to decide whether it received turn-by-turn narration sees narration it never requested, and a client iterating step.ExitNumber hits an absent list instead of the empty list AWS returns.
Fix: the CalculateRoutes travel-step builder in the geo-routes crate - gate Instruction/CurrentRoad/NextRoad on the request's TravelStepType and LegAdditionalFeatures, and always emit ExitNumber (empty when the step has no exit).
Traces: geo-routes-calculate-routes-truck-travel-mode-options-wiring-success#task#1, geo-routes-calculate-routes-zero-max-alternatives-returns-single-route#task#1.

Cause 4 of 5 - MajorRoadLabels always uses RoadName and never RouteNumber, and omits Language.
AWS: Smithy: RouteMajorRoadLabel is a union-like structure of two optional members, RoadName (LocalizedString) and RouteNumber (RouteNumber). LocalizedString and RouteNumber both require Value and both carry an optional Language (BCP 47, @length 2..35). The recording labels the route by its route number: MajorRoadLabels[0] = {RouteNumber: {Language: "en", Value: "I-5"}}, i.e. for a numbered highway the RouteNumber variant is populated, with Language set.
Emulator: it always emits the RoadName variant and never RouteNumber, and the LocalizedString it emits has no Language member at all - got {RoadName: {Value: "Motorway M95060"}} where the recording has {RouteNumber: {Language: "en", Value: "I-5"}}. The label text itself is generated and is not a requirement; the variant selection and the missing Language are.
Why: MajorRoadLabels exists so a client can tell alternatives apart in a UI. A client reading label.RouteNumber.Value to render a highway shield always gets null, and any consumer that relies on Language to pick a localized rendering has nothing to key on.
Fix: the CalculateRoutes route-summary builder in the geo-routes crate - choose the RouteNumber variant when the major road is numbered, and populate Language on the emitted LocalizedString/RouteNumber.
Traces: geo-routes-calculate-routes-truck-travel-mode-options-wiring-success#task#1, geo-routes-calculate-routes-zero-max-alternatives-returns-single-route#task#1.

Cause 5 of 5 - the x-amz-geo-pricing-bucket response header carries a request kind instead of a pricing bucket.
AWS: Smithy: CalculateRoutesResponse.PricingBucket is @required and bound to @httpHeader("x-amz-geo-pricing-bucket"), documented as "The pricing bucket for which the query is charged at." The recorded response sets it to "Core" (recorded behavior only for the value set; the recording is the authority on what a valid bucket token looks like).
Emulator: it returns "RoutesRequest" - the name of the request/operation class, not a pricing bucket: "$headers.x-amz-geo-pricing-bucket: value_mismatch (expected 'Core', got 'RoutesRequest')".
Why: this header is how a caller attributes a call to a billing tier; cost-attribution and metering clients bucket every geo-routes call under a token AWS never emits, so their per-bucket totals are wrong and any allow-list of known buckets rejects the response.
Fix: the CalculateRoutes handler's response-header population in the geo-routes crate - set PricingBucket to the pricing bucket the request falls into rather than to the request type.
Traces: geo-routes-calculate-routes-truck-travel-mode-options-wiring-success#task#1, geo-routes-calculate-routes-zero-max-alternatives-returns-single-route#task#1.

## Evidence
Replay verdict: mismatch, service=geo-routes, operation=CalculateRoutes, expected_status=200, actual_status=200. Cluster affects 2 fixtures (geo-routes-calculate-routes-truck-travel-mode-options-wiring-success#task#1, geo-routes-calculate-routes-zero-max-alternatives-returns-single-route#task#1). Per-path detail as reported by the replay:

$.Routes[0].Legs[0].VehicleLegDetails.Arrival.Place.OriginalPosition[0]: value_mismatch (expected -122.3088001, got -122.3088)
$.Routes[0].Legs[0].VehicleLegDetails.Arrival.Place.OriginalPosition[1]: value_mismatch (expected 47.4501999, got 47.4502)
$.Routes[0].Legs[0].VehicleLegDetails.Arrival.Place.Position[0]: value_mismatch (expected -122.3029601, got -122.3088)
$.Routes[0].Legs[0].VehicleLegDetails.Arrival.Place.Position[1]: value_mismatch (expected 47.45124, got 47.4502)
$.Routes[0].Legs[0].VehicleLegDetails.Departure.Place.OriginalPosition[0]: value_mismatch (expected -122.3410001, got -122.341)
$.Routes[0].Legs[0].VehicleLegDetails.Departure.Place.Position[0]: value_mismatch (expected -122.3409556, got -122.341)
$.Routes[0].Legs[0].VehicleLegDetails.Departure.Place.Position[1]: value_mismatch (expected 47.6089359, got 47.6089)
$.Routes[0].Legs[0].VehicleLegDetails.Summary: missing (expected {'Overview': {'BestCaseDuration': 1344, 'Distance': 23113, 'Duration': 1344}, 'TravelOnly': {'BestCaseDuration': 1344, 'Duration': 1344}}, got None)
$.Routes[0].Legs[0].VehicleLegDetails.TravelSteps[0]: value_mismatch (expected {'Distance': 83, 'Duration': 36, 'ExitNumber': [], 'GeometryOffset': 0, 'Type': 'Depart'}, got {'CurrentRoad': {'RoadName': [{'Language': 'en', 'Value': 'Meridian Street -244678'}], 'RouteNumber': [], 'Towards': [], 'Type': 'Urban'}, 'Distance': 1487, 'Duration': 134, 'GeometryOffset': 0, 'Instruction': 'Depart on Meridian Street -244678 and continue for 1.5 km.', 'NextRoad': {'RoadName': [{'Language': 'en', 'Value': 'Meridian Street -244674'}], 'RouteNumber': [], 'Towards': [], 'Type': 'Urban'}, 'Type': 'Depart'})
$.Routes[0].MajorRoadLabels[0]: value_mismatch (expected {'RouteNumber': {'Language': 'en', 'Value': 'I-5'}}, got {'RoadName': {'Value': 'Motorway M95060'}})
$headers.x-amz-geo-pricing-bucket: value_mismatch (expected 'Core', got 'RoutesRequest')

Model citations used: CalculateRoutes POST /v2/routes, output PricingBucket @required @httpHeader("x-amz-geo-pricing-bucket"); RouteVehicleLegDetails.Summary -> RouteVehicleSummary{Overview,TravelOnly} with RouteVehicleOverviewSummary.Distance/@required and .Duration/@required; RouteVehiclePlace.OriginalPosition/.Position -> Position23 (Double, @length 2..3), OriginalPosition documented "Position provided in the request"; RouteVehicleTravelStep requires only Duration+Type, Instruction documented "Only available when the TravelStepType is Default", ExitNumber -> LocalizedStringList; RouteMajorRoadLabel{RoadName: LocalizedString, RouteNumber: RouteNumber}, both carrying optional Language (@length 2..35) and @required Value. Generated road names, generated ids and the per-step distance/duration magnitudes are quoted as examples only, not as requirements.

## Steps to Reproduce
1. git clone the mainline checkout and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 against it (replay_wire_corpus).
3. Fixture geo-routes-calculate-routes-truck-travel-mode-options-wiring-success, step task#1: POST /v2/routes (CalculateRoutes) with a truck TravelMode plus TravelModeOptions, an Origin and a Destination given at 7-decimal precision.
4. Compare the response against the recording: the response is 200 but (a) every Place OriginalPosition/Position is rounded to 4 decimals and Position equals OriginalPosition, (b) Routes[0].Legs[0].VehicleLegDetails.Summary is absent, (c) TravelSteps[0] carries Instruction/CurrentRoad/NextRoad that were not requested and omits ExitNumber, (d) MajorRoadLabels[0] uses RoadName (no Language) instead of RouteNumber, (e) the x-amz-geo-pricing-bucket header is "RoutesRequest" instead of a pricing bucket such as "Core".
5. The same cluster reproduces on geo-routes-calculate-routes-zero-max-alternatives-returns-single-route, step task#1.
