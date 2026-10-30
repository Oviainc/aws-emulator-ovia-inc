Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: geo-routes.calculate-routes-max-alternatives-out-of-range-rejected geo-routes.calculate-routes-too-many-languages-rejected geo-routes.calculate-isolines-too-many-thresholds-rejected geo-routes.calculate-routes-malformed-departure-time-rejected geo-routes.calculate-routes-missing-destination-rejected geo-routes.snap-to-roads-too-few-points-rejected geo-routes.snap-to-roads-invalid-latitude-rejected geo-routes.snap-to-roads-snap-radius-out-of-range-rejected geo-routes.calculate-route-matrix-empty-origins-rejected geo-routes.optimize-waypoints-duplicate-ids-rejected geo-routes.calculate-routes-conflicting-travel-mode-options-rejected geo-routes.calculate-route-matrix-travel-mode-options-mismatch-rejected geo-routes.optimize-waypoints-origin-only-minimal-success
Issue-Shape-Id: com.amazonaws.georoutes#RoutesService
Issue-Key: geo-routes.calculate-routes-max-alternatives-out-of-range-rejected|run#4|AssertionError|assert error["Message"] == "'MaxAlternatives' must not be greater than 5", (
Issue-Key: geo-routes.calculate-routes-too-many-languages-rejected|run#4|AssertionError|assert error["Message"] == "'Languages' must have length at most 10", (
Issue-Key: geo-routes.calculate-isolines-too-many-thresholds-rejected|run#4|AssertionError|assert error["Message"] == "'Thresholds.Time' must have length at most 5", (
Issue-Key: geo-routes.calculate-routes-malformed-departure-time-rejected|run#4|AssertionError|assert error["Message"] == (
Issue-Key: geo-routes.calculate-routes-missing-destination-rejected|run#4|AssertionError|assert error["Message"] == (
Issue-Key: geo-routes.snap-to-roads-too-few-points-rejected|run#4|AssertionError|assert error["Message"] == (
Issue-Key: geo-routes.snap-to-roads-invalid-latitude-rejected|run#5|AssertionError|assert error["Message"] == (
Issue-Key: geo-routes.snap-to-roads-snap-radius-out-of-range-rejected|run#4|AssertionError|assert error["Message"] == "'SnapRadius' must not be greater than 10000", (
Issue-Key: geo-routes.calculate-route-matrix-empty-origins-rejected|run#7|AssertionError|assert error["Message"] == (
Issue-Key: geo-routes.optimize-waypoints-duplicate-ids-rejected|run#5|AssertionError|assert error["Message"] == (
Issue-Key: geo-routes.calculate-routes-conflicting-travel-mode-options-rejected|_client_error#2|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: geo-routes.calculate-route-matrix-travel-mode-options-mismatch-rejected|_client_error#2|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: geo-routes.optimize-waypoints-origin-only-minimal-success|_client_error#2|AssertionError|raise AssertionError(f"expected ValidationException, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# geo-routes request validation: refusal messages use non-AWS wording and five requests AWS refuses are accepted with 200

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CalculateRoutes

## Description
This issue bundles 8 net-new root causes in the geo-routes request-validation path, found by replaying the behavioral suite simparity-behavioral-aws-v1. Causes 1-5 are refusals that fire with the right status (400) and error code (ValidationException) but a message worded differently from AWS. Causes 6-8 are requests real AWS refuses that the emulator ACCEPTS with 200 and a synthesized result. A fixer must close ALL EIGHT causes before submitting.

Related already-open issue (wire corpus, NOT a duplicate and not marked as owning these behavioral checks): 20261029T172141Z-geo-routes-validationexception-refusals-use-emulator-prose-instead.md reports the same wording family and the same Origin-only accept from recorded wire fixtures; the checks listed here are the behavioral replay keys for them and are not carried by that issue.

Cause 1 of 8: refusals of modelled constraints (@range/@length/@pattern) are rendered in the generic AWS constraint-violation format instead of geo-routes' own sentence.
AWS: Smithy: CalculateRoutesRequest.MaxAlternatives targets Integer @range min 0 max 5; CalculateRoutesRequest.Languages @length max 10; CalculateRoutesRequest.DepartureTime targets TimestampWithTimezoneOffset @pattern ^([1-2][0-9]{3})-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):([0-5][0-9]):([0-5][0-9]|60)(\.[0-9]{0,9})?(Z|[+-]([01][0-9]|2[0-3]):[0-5][0-9])$; IsolineThresholds.Time @length min 1 max 5; error ValidationException (httpError 400, Message/Reason/FieldList all @required). The model carries the constraints but not the message text, so: recorded behavior - each refusal is HTTP 400 ValidationException whose Message is geo-routes' own one-sentence form that names the member in single quotes in its MODELLED (PascalCase, dotted for nested members) spelling and states the violated bound, and does NOT quote the rejected value: "'MaxAlternatives' must not be greater than 5" (range max), "'Languages' must have length at most 10" (length max), "'Thresholds.Time' must have length at most 5" (nested length max), "'DepartureTime' must match regex <the member's @pattern>". The fixed text is the sentence template; the variable parts are the member path taken from the request and the limit/pattern taken from the model.
Emulator: returns 400 ValidationException with the generic framework format, quoting the rejected value and using lower-camel wire member names: "1 validation error detected: Value '6' at 'maxAlternatives' failed to satisfy constraint: Member must have value less than or equal to 5"; "1 validation error detected: Value '11 items' at 'languages' failed to satisfy constraint: Member must have length less than or equal to 10"; "1 validation error detected: Value '6 items' at 'thresholds.time' failed to satisfy constraint: Member must have length less than or equal to 5"; "1 validation error detected: Value 'not-a-timestamp' at 'departureTime' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([1-2][0-9]{3})-...$".
Why: clients, conformance tests and operator tooling match on AWS's sentence and parse the member name out of it; the generic format fails those matches, names members by a spelling the SDK never shows the caller, and leaks the rejected value into a message AWS keeps value-free.
Fix: the geo-routes crate's shared constraint-validation message formatter (the component that renders @range/@length/@pattern violations into ValidationException.Message and FieldList) - geo-routes must not use the generic 'N validation error detected' rendering.
Traces: geo-routes.calculate-routes-max-alternatives-out-of-range-rejected; geo-routes.calculate-routes-too-many-languages-rejected; geo-routes.calculate-isolines-too-many-thresholds-rejected; geo-routes.calculate-routes-malformed-departure-time-rejected. (Note: the isolines check's positive control afterwards calls CalculateIsolines, which is an unimplemented scaffold already covered by its own filed issue; that part is out of scope here.)

Cause 2 of 8: out-of-range coordinate refusals are worded in emulator prose that inlines a member path and an interval.
AWS: Smithy: Position is a list of Double @length 2..2, documented as 'World Geodetic System (WGS 84) format: [longitude, latitude]' with no numeric @range, so the domain check is server-side only. Recorded behavior: HTTP 400 ValidationException whose message is fixed text stating the component, its bounds and the rejected value: 'Longitude must be between -180 and 180 degrees, but was set to <value>.' and 'Latitude must be between -90 and 90 degrees, but was set to <value>.' Fixed parts: the component name, the bounds, the sentence form including the trailing period; the only variable is the rejected coordinate taken from the request. No member path is interpolated.
Emulator: returns 400 with interval-notation prose and the member path inlined, e.g. 'Destination longitude must be in [-180, 180]; got 200' (CalculateRoutes), 'TracePoints[1].Position longitude must be in [-180, 180]; got 200' and 'TracePoints[1].Position latitude must be in [-90, 90]; got 200' (SnapToRoads).
Why: status and error class are right but the message a client surfaces or matches on is not AWS's, so message-based assertions and user-facing error text diverge for the single most common malformed-input case.
Fix: the shared coordinate/position range-validation component used by the CalculateRoutes and SnapToRoads handlers (and the crate's ValidationException message builder).
Traces: geo-routes.calculate-routes-missing-destination-rejected; geo-routes.snap-to-roads-too-few-points-rejected; geo-routes.snap-to-roads-invalid-latitude-rejected.

Cause 3 of 8: the SnapRadius out-of-range refusal is worded in emulator prose.
AWS: Smithy: SnapToRoadsRequest.SnapRadius targets DistanceMeters @range min 0 max 10000 (default 0, unit meters). Recorded behavior: HTTP 400 ValidationException with the same range-max template as Cause 1: "'SnapRadius' must not be greater than 10000" - fixed sentence naming the member in single quotes and the modelled upper bound, with no units and no echo of the rejected value.
Emulator: returns 400 with 'SnapRadius must be in [0, 10000] metres; got 10001' - unquoted member, interval notation, a unit word and the rejected value appended.
Why: same as Cause 2 - a correct refusal that no AWS-matching client recognizes.
Fix: the SnapToRoads handler's SnapRadius validation, routed through the crate's ValidationException message builder.
Traces: geo-routes.snap-to-roads-snap-radius-out-of-range-rejected.

Cause 4 of 8: the CalculateRouteMatrix cell-count cap refusal is worded in emulator prose and echoes the computed matrix size.
AWS: Smithy: CalculateRouteMatrixRequest.Origins and .Destinations are @required @length min 1 lists whose docs state 'The total matrix size (origins x destinations) must not exceed: ... With RoutingBoundary.Unbounded: 100'; the product cap itself is not a model constraint. Recorded behavior: HTTP 400 ValidationException whose message is fixed text stating the cap and the boundary mode it applies under: "The total number of routes to calculate must be less than or equal to 100 when 'RoutingBoundary.Unbounded' has value true." Variable parts: the limit and the named boundary member/mode; the offending product is NOT quoted. The same sentence is repeated in fieldList[0].message.
Emulator: returns 400 with 'the total matrix size must not exceed 100 for this routing boundary; got 110' - lower-case prose, no reference to RoutingBoundary.Unbounded, and it appends the computed cell count.
Why: the caller cannot tell from the message which boundary mode imposed the 100-route cap, so it cannot learn that a bounded request raises the limit; message matching also fails.
Fix: the CalculateRouteMatrix handler's matrix-size validation and the crate's ValidationException message builder.
Traces: geo-routes.calculate-route-matrix-empty-origins-rejected (the check sends an 11x10=110 unbounded matrix despite its id; Origins=[] is refused client-side by botocore and never reaches the emulator).

Cause 5 of 8: the duplicate-waypoint-Id refusal is worded in emulator prose and interpolates the duplicated id.
AWS: Smithy: WaypointOptimizationWaypoint.Id targets WaypointId @length 1..100 @pattern ^[^,;!]+$; cross-member uniqueness is not modelled. Recorded behavior: HTTP 400 ValidationException whose message is entirely fixed text naming the three members the ids must be unique across: "'Id' must be unique across 'OriginOptions', 'Waypoints' and 'DestinationOptions'." No request value is interpolated.
Emulator: returns 400 with 'waypoint ids must be unique within a request; dup is used more than once', i.e. emulator prose that interpolates the offending id value from the request.
Why: the message is what a client shows and matches; echoing a request value where AWS keeps the sentence fixed also makes the text non-deterministic across requests.
Fix: the OptimizeWaypoints handler's waypoint-id uniqueness validation and the crate's ValidationException message builder.
Traces: geo-routes.optimize-waypoints-duplicate-ids-rejected.

Cause 6 of 8: CalculateRoutes ACCEPTS TravelModeOptions for a travel mode other than the selected TravelMode.
AWS: Smithy: CalculateRoutesRequest.TravelMode targets RouteTravelMode and .TravelModeOptions targets RouteTravelModeOptions, whose members are Car, Pedestrian, Scooter, Truck, Intermodal and Transit - a union-like structure whose per-mode members the model cannot constrain against TravelMode, and CalculateRoutes models ValidationException (httpError 400). Recorded behavior: a request with TravelMode=Car and TravelModeOptions.Truck set is refused HTTP 400 ValidationException with the message "Only the following fields may be set within 'TravelModeOptions' when 'TravelMode' has value Car: 'Car', 'Ev'." Fixed text is the sentence; the variable parts are the selected TravelMode value from the request and the member list permitted for that mode.
Emulator: answers HTTP 200 with a fully synthesized route (one Car Vehicle leg, Summary/TravelSteps/Polyline populated, PricingBucket 'Core'), silently ignoring the mismatched options.
Why: a client that builds a request from a mode selector plus a per-mode options form gets a plausible route computed WITHOUT the truck constraints it supplied - the exact bug AWS's refusal exists to catch - so a truck is routed as a car (low bridges, weight-restricted roads) while the caller believes its options were honoured.
Fix: the CalculateRoutes handler's input-validation component - add the TravelMode/TravelModeOptions cross-member check, raising the same ValidationException builder the other refusals use.
Traces: geo-routes.calculate-routes-conflicting-travel-mode-options-rejected (expected 400, got 200).

Cause 7 of 8: CalculateRouteMatrix ACCEPTS Truck options under TravelMode=Pedestrian.
AWS: Smithy: CalculateRouteMatrixRequest.TravelModeOptions targets RouteMatrixTravelModeOptions, which defines ONLY Car, Scooter and Truck, while RouteMatrixTravelMode admits Pedestrian - so Truck options under a Pedestrian matrix is an unsatisfiable combination; the operation models ValidationException (httpError 400). Recorded behavior: the request is refused HTTP 400 ValidationException with the message "'TravelModeOptions.Truck' must not be set when 'TravelMode' has value Pedestrian." Fixed text is the sentence; the variable parts are the offending option member path and the TravelMode value, both taken from the request.
Emulator: answers HTTP 200 with RouteMatrix [[{Distance, Duration}]], ErrorCount 0 and RoutingBoundary {Unbounded: true}.
Why: the caller gets a matrix silently computed for a different travel mode than the constraints it sent, and no signal that the option block was discarded.
Fix: the CalculateRouteMatrix handler's input-validation component (same cross-member rule as Cause 6, over RouteMatrixTravelModeOptions' smaller member set).
Traces: geo-routes.calculate-route-matrix-travel-mode-options-mismatch-rejected (expected 400, got 200).

Cause 8 of 8: OptimizeWaypoints ACCEPTS a request with neither Destination nor Waypoints.
AWS: Smithy: OptimizeWaypointsRequest.Origin is @required while Destination (Position) and Waypoints (WaypointOptimizationWaypointList) are both optional, so the 'at least one of them' rule is not expressible in the model; the operation models ValidationException (httpError 400) whose Message, Reason (ValidationExceptionReason) and FieldList are all @required. Recorded behavior: an Origin-only request is refused HTTP 400 ValidationException with Reason FieldValidationFailed and the fixed-text message naming the mutually-sufficient members: 'At least one of the following fields must be set: Destination, Waypoints.' The only variable part is that member list, which is this operation's optional-destination pair; fieldList[0] names the first of those members with the same sentence as its message.
Emulator: answers HTTP 200 with Connections [], Distance 0, Duration 0, ImpedingWaypoints [], a single OptimizedWaypoints entry with Id 'Origin', and an all-zero TimeBreakdown.
Why: a client relying on AWS rejecting a half-built request (a form that has not collected its stops, a retry whose waypoint list was lost) stores a zero-length 'optimized' route to nowhere instead of seeing a 400.
Fix: the OptimizeWaypoints handler's input-validation component - add the 'at least one of Destination, Waypoints' check alongside the Origin requirement.
Traces: geo-routes.optimize-waypoints-origin-only-minimal-success (expected 400 ValidationException, got 200).

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator: 28/62 checks passed, all 34 failures in geo-routes.

Cause 1 (status 400 and code ValidationException correct, message wrong):
  geo-routes.calculate-routes-max-alternatives-out-of-range-rejected (MaxAlternatives=6): expected "'MaxAlternatives' must not be greater than 5", got "1 validation error detected: Value '6' at 'maxAlternatives' failed to satisfy constraint: Member must have value less than or equal to 5"
  geo-routes.calculate-routes-too-many-languages-rejected (11 Languages): expected "'Languages' must have length at most 10", got "1 validation error detected: Value '11 items' at 'languages' failed to satisfy constraint: Member must have length less than or equal to 10"
  geo-routes.calculate-isolines-too-many-thresholds-rejected (6 Time thresholds): expected "'Thresholds.Time' must have length at most 5", got "1 validation error detected: Value '6 items' at 'thresholds.time' failed to satisfy constraint: Member must have length less than or equal to 5"
  geo-routes.calculate-routes-malformed-departure-time-rejected (DepartureTime='not-a-timestamp'): expected "'DepartureTime' must match regex ^([1-2][0-9]{3})-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):([0-5][0-9]):([0-5][0-9]|60)(\.[0-9]{0,9})?(Z|[+-]([01][0-9]|2[0-3]):[0-5][0-9])$", got "1 validation error detected: Value 'not-a-timestamp' at 'departureTime' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([1-2][0-9]{3})-...$"

Cause 2:
  geo-routes.calculate-routes-missing-destination-rejected (Destination=[200.0, 100.0]): expected 'Longitude must be between -180 and 180 degrees, but was set to 200.', got 'Destination longitude must be in [-180, 180]; got 200'
  geo-routes.snap-to-roads-too-few-points-rejected (TracePoints[1].Position=[200.0, 47.6110]): expected 'Longitude must be between -180 and 180 degrees, but was set to 200.', got 'TracePoints[1].Position longitude must be in [-180, 180]; got 200'
  geo-routes.snap-to-roads-invalid-latitude-rejected (TracePoints[1].Position=[-122.3363, 200.0]): expected 'Latitude must be between -90 and 90 degrees, but was set to 200.', got 'TracePoints[1].Position latitude must be in [-90, 90]; got 200'

Cause 3:
  geo-routes.snap-to-roads-snap-radius-out-of-range-rejected (SnapRadius=10001): expected "'SnapRadius' must not be greater than 10000", got 'SnapRadius must be in [0, 10000] metres; got 10001'

Cause 4:
  geo-routes.calculate-route-matrix-empty-origins-rejected (11 Origins x 10 Destinations, RoutingBoundary.Unbounded=true): expected "The total number of routes to calculate must be less than or equal to 100 when 'RoutingBoundary.Unbounded' has value true.", got 'the total matrix size must not exceed 100 for this routing boundary; got 110'

Cause 5:
  geo-routes.optimize-waypoints-duplicate-ids-rejected (two Waypoints with Id 'dup'): expected "'Id' must be unique across 'OriginOptions', 'Waypoints' and 'DestinationOptions'.", got 'waypoint ids must be unique within a request; dup is used more than once'

Cause 6:
  geo-routes.calculate-routes-conflicting-travel-mode-options-rejected (TravelMode=Car, TravelModeOptions={'Truck': {'GrossWeight': 10000}}): expected 400 ValidationException "Only the following fields may be set within 'TravelModeOptions' when 'TravelMode' has value Car: 'Car', 'Ev'.", got HTTP 200 with PricingBucket 'Core', LegGeometryFormat 'FlexiblePolyline' and a full synthesized Car route (Summary.Distance 1568, Duration 113).

Cause 7:
  geo-routes.calculate-route-matrix-travel-mode-options-mismatch-rejected (TravelMode=Pedestrian, TravelModeOptions={'Truck': {'GrossWeight': 10000}}): expected 400 ValidationException "'TravelModeOptions.Truck' must not be set when 'TravelMode' has value Pedestrian.", got HTTP 200 {'PricingBucket': 'Core', 'ErrorCount': 0, 'RouteMatrix': [[{'Distance': 17834, 'Duration': 12739}]], 'RoutingBoundary': {'Unbounded': True}}.

Cause 8:
  geo-routes.optimize-waypoints-origin-only-minimal-success (OptimizeWaypoints with Origin only): expected 400 ValidationException 'At least one of the following fields must be set: Destination, Waypoints.', got HTTP 200 {'PricingBucket': 'Core', 'Connections': [], 'Distance': 0, 'Duration': 0, 'ImpedingWaypoints': [], 'OptimizedWaypoints': [{'Id': 'Origin', ...}], 'TimeBreakdown': {'RestDuration': 0, 'ServiceDuration': 0, 'TravelDuration': 0, 'WaitDuration': 0}}.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the behavioral suite simparity-behavioral-aws-v1 (service geo-routes).
2. Cause 1: replay geo-routes.calculate-routes-max-alternatives-out-of-range-rejected, geo-routes.calculate-routes-too-many-languages-rejected, geo-routes.calculate-isolines-too-many-thresholds-rejected and geo-routes.calculate-routes-malformed-departure-time-rejected; for each, POST the over-constraint request and compare the Error.Message of the 400 against the AWS sentence quoted above.
3. Cause 2: replay geo-routes.calculate-routes-missing-destination-rejected (POST /v2/routes with Destination=[200.0, 100.0]), geo-routes.snap-to-roads-too-few-points-rejected and geo-routes.snap-to-roads-invalid-latitude-rejected (POST /v2/snap-to-roads with one out-of-domain coordinate) and compare Error.Message.
4. Cause 3: replay geo-routes.snap-to-roads-snap-radius-out-of-range-rejected (SnapRadius=10001) and compare Error.Message.
5. Cause 4: replay geo-routes.calculate-route-matrix-empty-origins-rejected (POST /v2/route-matrix, 11 Origins x 10 Destinations, RoutingBoundary.Unbounded=true) and compare Error.Message.
6. Cause 5: replay geo-routes.optimize-waypoints-duplicate-ids-rejected (POST /v2/optimize-waypoints with two Waypoints sharing Id 'dup') and compare Error.Message.
7. Cause 6: replay geo-routes.calculate-routes-conflicting-travel-mode-options-rejected - POST /v2/routes with TravelMode=Car and TravelModeOptions.Truck; the emulator answers 200 where a 400 ValidationException is due.
8. Cause 7: replay geo-routes.calculate-route-matrix-travel-mode-options-mismatch-rejected - POST /v2/route-matrix with TravelMode=Pedestrian and TravelModeOptions.Truck; the emulator answers 200.
9. Cause 8: replay geo-routes.optimize-waypoints-origin-only-minimal-success - POST /v2/optimize-waypoints with Origin only (no Destination, no Waypoints); the emulator answers 200.
