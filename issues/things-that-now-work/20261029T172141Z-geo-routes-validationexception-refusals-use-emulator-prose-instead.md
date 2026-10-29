Issue-Source: trace-sweep-divergence
Issue-Trace: geo-routes-optimize-waypoints-origin-only-rejected
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: geo-routes-optimize-waypoints-origin-only-rejected geo-routes-calculate-route-matrix-cap-exceeded-rejected geo-routes-optimize-waypoints-duplicate-ids-rejected geo-routes-snap-to-roads-invalid-coordinate-rejected geo-routes-snap-to-roads-snap-radius-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.georoutes#RoutesService
Issue-Key: geo-routes|OptimizeWaypoints|4xx->2xx|$.fieldList
Issue-Key: geo-routes|OptimizeWaypoints|4xx->2xx|$.message
Issue-Key: geo-routes|OptimizeWaypoints|4xx->2xx|$.reason
Issue-Key: geo-routes|OptimizeWaypoints|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: geo-routes|OptimizeWaypoints|4xx->2xx|status
Issue-Key: geo-routes|CalculateRouteMatrix|4xx->4xx|$.fieldList[*].message
Issue-Key: geo-routes|CalculateRouteMatrix|4xx->4xx|$.message
Issue-Key: geo-routes|OptimizeWaypoints|4xx->4xx|$.fieldList[*]
Issue-Key: geo-routes|OptimizeWaypoints|4xx->4xx|$.message
Issue-Key: geo-routes|SnapToRoads|4xx->4xx|$.fieldList[*].message
Issue-Key: geo-routes|SnapToRoads|4xx->4xx|$.message
Issue-Commit: 4fccfcb9e83e9e12ca223d9fc117bac8a0390f59

# geo-routes ValidationException refusals use emulator prose instead of AWS wording, and one required-field check is missing entirely

> _Note: this issue was identified against the code base at commit `4fccfcb9e83e9e12ca223d9fc117bac8a0390f59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
OptimizeWaypoints

## Description
This issue bundles 4 net-new root causes in geo-routes request validation: one request AWS refuses is ACCEPTED, and three refusals that do fire are worded in the emulator's own prose (and one of them names the wrong field) instead of AWS's ValidationException wording. Status and error class are correct for the latter three; the message, and in one case fieldList[0].name, are not. A fixer must close ALL FOUR causes before submitting.

Cause 1 of 4: OptimizeWaypoints accepts a request with neither Destination nor Waypoints and answers 200.
AWS: Smithy: OptimizeWaypointsRequest.Origin is @required while Destination (Position) and Waypoints (WaypointOptimizationWaypointList) are both optional members, so the 'at least one of them' rule is not expressible in the model; recorded behavior: an Origin-only request is refused with HTTP 400 ValidationException carrying reason = FieldValidationFailed (Smithy: ValidationException.Reason targets enum ValidationExceptionReason whose members include FieldValidationFailed), the x-amzn-errortype header, and fieldList = [{name, message}] (Smithy: ValidationException.FieldList @required, ValidationExceptionField.Name/.Message @required). The message is fixed text naming the mutually-sufficient members: 'At least one of the following fields must be set: Destination, Waypoints.' - the only variable part is the member list itself, which is this operation's optional-destination pair, and fieldList[0].name is the first of those members with the same sentence as its message.
Emulator: answers 200 with a synthesized single-connection optimization result; $.message, $.reason, $.fieldList and the errortype header are all absent.
Why: a client that relies on AWS rejecting a half-built request (a form that has not collected its stops yet, a retry whose waypoint list was lost) gets a plausible success with a fabricated route instead of a 400, so the bug is silently accepted and the client stores a route to nowhere.
Fix: the OptimizeWaypoints handler's input-validation component - add the 'at least one of Destination, Waypoints' check alongside the Origin requirement, raising the same ValidationException builder the other refusals use.
Traces: geo-routes-optimize-waypoints-origin-only-rejected#task#1 (expected 400, got 200).

Cause 2 of 4: the CalculateRouteMatrix cell-count cap refusal is worded in emulator prose and echoes the computed size.
AWS: Smithy: CalculateRouteMatrix errors include ValidationException (httpError 400); Origins/Destinations are @length min 1 lists and the cap on their product is not in the model, so: recorded behavior - the refusal message is fixed text that states the cap and the boundary mode it applies under: "The total number of routes to calculate must be less than or equal to 100 when 'RoutingBoundary.Unbounded' has value true." Its variable parts are the limit and the named boundary member/mode; the offending product is NOT quoted. The same sentence is repeated as fieldList[0].message.
Emulator: returns 400 with 'the total matrix size must not exceed 100 for this routing boundary; got 110' in both $.message and $.fieldList[0].message - lower-case emulator prose, no reference to RoutingBoundary.Unbounded, and it appends the computed cell count.
Why: operators and tests match on AWS's sentence (and on it being identical in message and fieldList); the emulator's wording fails those matches and tells the caller nothing about which boundary mode imposed the 100-route cap, so a caller cannot tell that switching to a bounded request raises the limit.
Fix: the CalculateRouteMatrix handler's validation component / the crate's ValidationException message builder.
Traces: geo-routes-calculate-route-matrix-cap-exceeded-rejected#task#1.

Cause 3 of 4: the duplicate-waypoint-Id refusal is worded in emulator prose AND fieldList[0].name points at the list instead of the offending element's Id member.
AWS: Smithy: WaypointOptimizationWaypoint.Id targets WaypointId (@length 1..100, @pattern ^[^,;!]+$); uniqueness across the three id-bearing inputs is not in the model, so: recorded behavior - the refusal is 400 ValidationException whose message is fixed text naming the three members the ids must be unique across: "'Id' must be unique across 'OriginOptions', 'Waypoints' and 'DestinationOptions'." No request value is interpolated. fieldList[0].name is the path of the offending element's Id member, i.e. the list name with the duplicate element's position plus '.Id' (the position comes from the request), and fieldList[0].message repeats the same sentence.
Emulator: returns 400 with message 'waypoint ids must be unique within a request; dup is used more than once' - emulator prose that interpolates the duplicated id value, which AWS's wording does not do - and fieldList[0] = {name: 'Waypoints', message: <same prose>}, naming the whole list with no element position and no '.Id' member.
Why: fieldList exists so a client can highlight the exact offending input; naming the list leaves a client unable to point at the duplicate, and the non-AWS sentence breaks message matching. Echoing the id also leaks a request value into a message AWS keeps fixed.
Fix: the OptimizeWaypoints handler's validation component - emit AWS's sentence and build the fieldList name from the offending element's index and member path.
Traces: geo-routes-optimize-waypoints-duplicate-ids-rejected#task#1.

Cause 4 of 4: the out-of-range coordinate refusal is worded in emulator prose.
AWS: Smithy: RoadSnapTracePoint.Position targets Position (list of Double, @length 2..2, @required); the -180..180 longitude domain is documented as WGS 84 [longitude, latitude] rather than modelled as a range, so: recorded behavior - the refusal is 400 ValidationException whose message is fixed text with the rejected coordinate substituted: 'Longitude must be between -180 and 180 degrees, but was set to <value>.' The fixed parts are the component name, the bounds and the sentence form; the only variable is the rejected value taken from the request. The same sentence is repeated as fieldList[0].message.
Emulator: returns 400 with 'TracePoints[1].Position longitude must be in [-180, 180]; got 200' in both $.message and $.fieldList[0].message - emulator prose in a different sentence form (interval notation, a 'got' suffix, the member path inlined into the message).
Why: clients and conformance tests match AWS's sentence; the emulator's wording fails them even though status and error class are right. (The same divergence is reported for the SnapRadius-out-of-range fixture, which clusters onto the same wording defect.)
Fix: the SnapToRoads handler's coordinate/range validation component / the crate's ValidationException message builder.
Traces: geo-routes-snap-to-roads-invalid-coordinate-rejected#task#1 (cluster also covers geo-routes-snap-to-roads-invalid-coordinate-rejected#task#2 and geo-routes-snap-to-roads-snap-radius-out-of-range-rejected#task#1).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (12 fixtures, 19 steps, 0 passed).

Cause 1 - geo-routes-optimize-waypoints-origin-only-rejected#task#1: status expected 400, got 200.
  $.message: missing (expected 'At least one of the following fields must be set: Destination, Waypoints.', got None)
  $.reason: missing (expected 'FieldValidationFailed', got None)
  $.fieldList: missing (expected [{'name': 'Destination', 'message': 'At least one of the following fields must be set: Destination, Waypoints.'}], got None)
  $headers.x-amzn-errortype: missing (expected shaped value, got None)

Cause 2 - geo-routes-calculate-route-matrix-cap-exceeded-rejected#task#1, expected 400 / actual 400:
  $.message: expected "The total number of routes to calculate must be less than or equal to 100 when 'RoutingBoundary.Unbounded' has value true.", got 'the total matrix size must not exceed 100 for this routing boundary; got 110'
  $.fieldList[0].message: same divergence

Cause 3 - geo-routes-optimize-waypoints-duplicate-ids-rejected#task#1, expected 400 / actual 400:
  $.message: expected "'Id' must be unique across 'OriginOptions', 'Waypoints' and 'DestinationOptions'.", got 'waypoint ids must be unique within a request; dup is used more than once'
  $.fieldList[0]: expected {'name': 'Waypoints[1].Id', 'message': "'Id' must be unique across 'OriginOptions', 'Waypoints' and 'DestinationOptions'."}, got {'name': 'Waypoints', 'message': 'waypoint ids must be unique within a request; dup is used more than once'}

Cause 4 - geo-routes-snap-to-roads-invalid-coordinate-rejected#task#1, expected 400 / actual 400:
  $.message: expected 'Longitude must be between -180 and 180 degrees, but was set to 200.', got 'TracePoints[1].Position longitude must be in [-180, 180]; got 200'
  $.fieldList[0].message: same divergence
  (cluster affected_count 3: also ...#task#2 and geo-routes-snap-to-roads-snap-radius-out-of-range-rejected#task#1)

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay geo-routes-optimize-waypoints-origin-only-rejected step task#1 - POST /v2/optimize-waypoints with Origin only (no Destination, no Waypoints). Expect 400 ValidationException with reason FieldValidationFailed; the emulator answers 200 with a synthesized result.
3. Cause 2: replay geo-routes-calculate-route-matrix-cap-exceeded-rejected step task#1 - POST /v2/route-matrix whose Origins x Destinations product exceeds 100 under an unbounded RoutingBoundary; compare $.message and $.fieldList[0].message.
4. Cause 3: replay geo-routes-optimize-waypoints-duplicate-ids-rejected step task#1 - a Waypoints list repeating one Id; compare $.message and $.fieldList[0].
5. Cause 4: replay geo-routes-snap-to-roads-invalid-coordinate-rejected step task#1 - a TracePoints entry whose longitude is outside -180..180; compare $.message and $.fieldList[0].message.
