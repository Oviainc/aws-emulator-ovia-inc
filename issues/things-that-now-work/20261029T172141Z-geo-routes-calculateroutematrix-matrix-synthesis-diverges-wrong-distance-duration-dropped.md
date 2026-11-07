Issue-Source: trace-sweep-divergence
Issue-Trace: geo-routes-calculate-route-matrix-basic-success
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: geo-routes-calculate-route-matrix-basic-success geo-routes-calculate-route-matrix-cap-exceeded-rejected geo-routes-calculate-route-matrix-per-entry-error-codes
Issue-Shape-Id: com.amazonaws.georoutes#RoutesService
Issue-Key: geo-routes|CalculateRouteMatrix|2xx->2xx|$.RouteMatrix[*][*].Distance
Issue-Key: geo-routes|CalculateRouteMatrix|2xx->2xx|$.RouteMatrix[*][*].Duration
Issue-Key: geo-routes|CalculateRouteMatrix|2xx->2xx|$.RouteMatrix[*][*].Error
Issue-Key: geo-routes|CalculateRouteMatrix|2xx->2xx|$.RoutingBoundary.Unbounded
Issue-Key: geo-routes|CalculateRouteMatrix|2xx->2xx|$.ErrorCount
Issue-Commit: 4fccfcb9e83e9e12ca223d9fc117bac8a0390f59

# CalculateRouteMatrix matrix synthesis diverges: wrong Distance/Duration, dropped per-entry errors and RoutingBoundary echo

> _Note: this issue was identified against the code base at commit `4fccfcb9e83e9e12ca223d9fc117bac8a0390f59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CalculateRouteMatrix

## Description
This issue bundles 4 net-new root causes in the CalculateRouteMatrix response path. All four are reached on a 200 answer: the emulator serves a matrix whose metrics, per-entry error reporting, error count and boundary echo all diverge from AWS. A fixer must close ALL FOUR causes before submitting.

Cause 1 of 4: RouteMatrix entry Distance/Duration are synthesized from a model that does not reproduce AWS's route metrics.
AWS: Smithy: CalculateRouteMatrix -> CalculateRouteMatrixResponse.RouteMatrix @required (list of RouteMatrixEntry); RouteMatrixEntry.Distance (DistanceMeters, long, @range 0..4294967295, @required) is the total travel distance for the origin/destination pair and RouteMatrixEntry.Duration (DurationSeconds, seconds, @required) its expected travel duration. The model does not state the routing algorithm, so: recorded behavior only - for a fixed set of Origins/Destinations the recorded answer is a deterministic pair of values per cell, and the recorded distance/duration pairs are mutually consistent (larger distance, proportionally larger duration at a road-network speed).
Emulator: for the same request it returns a SHORTER distance together with a LONGER duration in every cell (its implied average speed is far below the recorded one), so neither member matches and the two are inconsistent with each other. These are not per-run values: the request's coordinates are fixed in the fixture and the emulator's numbers are reproducible, they are simply a different distance/duration model.
Why: a client that sizes a delivery window, picks the nearest depot, or filters pairs by a duration budget gets a different cell ranking and different ETAs than against AWS; the duration inflation alone pushes cells past client-side time limits that AWS would satisfy.
Fix: the CalculateRouteMatrix handler's matrix-synthesis component (the per-cell distance/duration computation shared with the other route-metric producers), not the serializer.
Traces: geo-routes-calculate-route-matrix-basic-success#task#1 (2x2 matrix, every cell diverges); geo-routes-calculate-route-matrix-cap-exceeded-rejected#task#2 (5x5 matrix, all 25 cells diverge the same way).

Cause 2 of 4: the per-cell Error code is over-specified - the entry-level variant is emitted where AWS reports the pair-level one.
AWS: Smithy: RouteMatrixEntry.Error targets enum RouteMatrixErrorCode with values NoMatch, NoMatchDestination, NoMatchOrigin, NoRoute, OutOfBounds, OutOfBoundsDestination, OutOfBoundsOrigin, Other, Violation - i.e. the generic code and the origin-/destination-qualified codes are distinct members. Recorded behavior: for a cell rejected because it falls outside the request's RoutingBoundary, the recorded answer sets Error to the unqualified boundary code (OutOfBounds).
Emulator: sets the destination-qualified code (OutOfBoundsDestination) for that same cell. Hypothesis: the handler attributes the boundary failure to whichever endpoint it tested last instead of emitting the unqualified code when it cannot attribute the failure to one endpoint.
Why: clients branch on this enum (retry with a wider boundary vs. drop a single destination). A qualified code tells a client the destination is at fault when AWS said only that the pair is out of bounds, so the client drops a usable destination.
Fix: the CalculateRouteMatrix handler's per-cell error-classification component.
Traces: geo-routes-calculate-route-matrix-per-entry-error-codes#task#1.

Cause 3 of 4: the response omits the required RoutingBoundary echo.
AWS: Smithy: CalculateRouteMatrixResponse.RoutingBoundary targets RouteMatrixBoundary and is @required; RouteMatrixBoundary.Unbounded is a boolean. The recorded answer carries the boundary the request was evaluated under, including an explicit false Unbounded when the request used a bounded geometry.
Emulator: leaves RoutingBoundary (and therefore RoutingBoundary.Unbounded) absent from the response body.
Why: a required output member is missing, so a strict SDK/codegen client deserializing the response fails or sees a null where the model guarantees a value; a client that reads back the effective boundary to decide whether to widen it has nothing to read.
Fix: the CalculateRouteMatrix handler/response-builder, which must echo the resolved boundary (defaulting Unbounded explicitly rather than omitting it).
Traces: geo-routes-calculate-route-matrix-per-entry-error-codes#task#1.

Cause 4 of 4: unroutable cells are answered as successful routes, so their Error is absent and ErrorCount stays 0.
AWS: Smithy: CalculateRouteMatrixResponse.ErrorCount (Integer, @range min 0, @required) is the number of matrix entries that failed; RouteMatrixEntry.Error carries NoRoute for a pair with no road connection. Recorded behavior: the cells with no connecting route carry Error = NoRoute and ErrorCount equals the number of such cells.
Emulator: emits no Error on those cells at all and reports ErrorCount 0, inventing distance/duration for pairs AWS declared unroutable (and the routable cells in the same matrix also carry the Cause-1 metric divergence).
Why: a client cannot tell a failed cell from a succeeded one: it reads a plausible distance/duration for an origin/destination pair that is genuinely unroutable, and the ErrorCount it uses as a cheap 'any failures?' check says the matrix is clean.
Fix: the CalculateRouteMatrix handler's per-cell routing path (emit the Error code when no route exists and derive ErrorCount from the entries that carry one).
Traces: geo-routes-calculate-route-matrix-per-entry-error-codes#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (12 fixtures, 19 steps, 0 passed).

Cause 1 - geo-routes-calculate-route-matrix-basic-success#task#1, expected 200 / actual 200:
  $.RouteMatrix[0][0].Distance: expected 22249, got 17458
  $.RouteMatrix[0][0].Duration: expected 976, got 1256
  $.RouteMatrix[0][1].Distance: expected 54104, got 40193
  $.RouteMatrix[0][1].Duration: expected 2047, got 2892
  $.RouteMatrix[1][0].Distance: expected 27518, got 19707
  $.RouteMatrix[1][0].Duration: expected 1247, got 1418
  $.RouteMatrix[1][1].Distance: expected 59553, got 43893
  $.RouteMatrix[1][1].Duration: expected 2354, got 3158
  (distance always short, duration always long -> different speed model)
  geo-routes-calculate-route-matrix-cap-exceeded-rejected#task#2, expected 200 / actual 200: all 25 cells diverge, e.g. $.RouteMatrix[0][0].Distance expected 172242 got 133704; $.RouteMatrix[0][0].Duration expected 7918 got 9619.

Cause 2 - geo-routes-calculate-route-matrix-per-entry-error-codes#task#1, expected 200 / actual 200:
  $.RouteMatrix[0][0].Error: expected 'OutOfBounds', got 'OutOfBoundsDestination'

Cause 3 - same step:
  $.RoutingBoundary.Unbounded: missing (expected False, got None)

Cause 4 - geo-routes-calculate-route-matrix-per-entry-error-codes#task#2, expected 200 / actual 200:
  $.ErrorCount: expected 2, got 0
  $.RouteMatrix[1][0].Error: missing (expected 'NoRoute', got None)
  $.RouteMatrix[1][1].Error: missing (expected 'NoRoute', got None)
  $.RouteMatrix[0][0].Distance: expected 280490, got 234016; $.RouteMatrix[0][0].Duration: expected 9425, got 16836

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture geo-routes-calculate-route-matrix-basic-success step task#1 (POST /v2/route-matrix with 2 Origins x 2 Destinations) and compare every $.RouteMatrix[i][j].Distance/.Duration against the recording; repeat with geo-routes-calculate-route-matrix-cap-exceeded-rejected step task#2 (5x5).
3. Causes 2 and 3: replay geo-routes-calculate-route-matrix-per-entry-error-codes step task#1 (bounded RoutingBoundary with a pair outside it) and inspect $.RouteMatrix[0][0].Error and $.RoutingBoundary.
4. Cause 4: replay geo-routes-calculate-route-matrix-per-entry-error-codes step task#2 (origin/destination pairs with no connecting route) and inspect $.ErrorCount and the Error member of the unroutable cells.
