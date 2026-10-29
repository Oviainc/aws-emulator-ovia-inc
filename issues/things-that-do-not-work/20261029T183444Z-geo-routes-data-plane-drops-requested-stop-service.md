Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: geo-routes.snap-to-roads-not-matched-notice geo-routes.snap-to-roads-notice-trace-points-not-matched geo-routes.snap-to-roads-unmatched-point-notice geo-routes.calculate-route-matrix-no-route-error-count-consistency geo-routes.calculate-route-matrix-out-of-bounds-destination geo-routes.calculate-routes-waypoint-stop-duration-increases-total-duration geo-routes.optimize-waypoints-service-duration-reflected geo-routes.calculate-routes-unsupported-language-triggers-fallback-notice geo-routes.optimize-waypoints-driving-distance-clustering-groups-nearby-waypoints
Issue-Shape-Id: com.amazonaws.georoutes#RoutesService
Issue-Key: geo-routes.snap-to-roads-not-matched-notice|run#10|AssertionError|assert _UNMATCHABLE_INDEX in flagged_indexes, f"no anomaly notice in {resp['Notices']}"
Issue-Key: geo-routes.snap-to-roads-notice-trace-points-not-matched|run#5|AssertionError|assert 2 in flagged_indexes, f"expected index 2 (open ocean) flagged, got notices {resp['Notices']}"
Issue-Key: geo-routes.snap-to-roads-unmatched-point-notice|run#6|AssertionError|assert _UNMATCHABLE_INDEX in flagged_indexes, f"no anomaly notice in {resp['Notices']}"
Issue-Key: geo-routes.calculate-route-matrix-no-route-error-count-consistency|run#8|AssertionError|assert entry.get("Error") == "NoRoute", (
Issue-Key: geo-routes.calculate-route-matrix-out-of-bounds-destination|run#9|AssertionError|assert error_code == "OutOfBounds", (
Issue-Key: geo-routes.calculate-routes-waypoint-stop-duration-increases-total-duration|run#7|AssertionError|assert abs(delta - STOP_SECONDS) < STOP_SECONDS * 0.2, (
Issue-Key: geo-routes.optimize-waypoints-service-duration-reflected|run#8|AssertionError|assert delta == _SERVICE_DURATION_SECONDS, (
Issue-Key: geo-routes.calculate-routes-unsupported-language-triggers-fallback-notice|run#4|AssertionError|assert "MainLanguageNotFound" not in supported_codes, (
Issue-Key: geo-routes|OptimizeWaypoints|NotImplemented
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# geo-routes data plane drops requested stop/service time, never emits SnapToRoads or language notices, mis-reports per-cell matrix errors, and 501s OptimizeWaypoints Clustering

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SnapToRoads

## Description
This issue bundles 6 net-new root causes in the geo-routes success-response/data-plane path. Every one is reached on a 2xx answer (except Cause 6, which is a NotImplemented refusal of a modelled input member). A fixer must close ALL SIX causes before submitting.

Related already-open issues (wire corpus; they do NOT carry these behavioral checks' replay keys): 20261029T172141Z-geo-routes-calculateroutematrix-matrix-synthesis-diverges-wrong-distance-duration-dropped.md covers the matrix metric/Error/ErrorCount family from recorded fixtures, and 20261029T172141Z-geo-routes-success-response-synthesis-diverges-across-calculateroutes-optimizewaypoints-and.md covers SnapToRoads map matching. Causes 3 and 4 below are the behavioral keys for the same defects; Causes 1, 2, 5 and 6 are not described by either.

Cause 1 of 6: SnapToRoads never emits a RoadSnapNotice, so a trace point that cannot be matched to any road is snapped silently.
AWS: Smithy: SnapToRoadsResponse.Notices is @required (RoadSnapNoticeList); RoadSnapNotice has Code (@required, enum RoadSnapNoticeCode with values TracePointsHeadingIgnored, TracePointsIgnored, TracePointsMovedByLargeDistance, TracePointsNotMatched, TracePointsOutOfSequence, TracePointsSpeedEstimated, TracePointsSpeedIgnored), Title (@required) and TracePointIndexes (@required, @length 1..1000) documented as 'TracePoint indices for which the provided notice code corresponds to'. Recorded behavior: a trace whose points are genuine road fixes except one in the open ocean comes back with a notice whose Code is TracePointsNotMatched (or TracePointsMovedByLargeDistance when the matcher moved it a huge distance) and whose TracePointIndexes contains exactly the index of that point - the on-road points are not listed.
Emulator: returns Notices: [] for every such request, under Car and Truck travel modes alike, while still returning a SnappedTracePoints entry for the unmatchable point.
Why: the notice list is the only channel telling a client which fixes were not really matched. With it always empty, a client accepts an ocean coordinate as a snapped road position and cannot filter or re-request the bad fixes; a required output member is effectively never populated.
Fix: the SnapToRoads handler's map-matching component must record per-point match outcomes and the response builder must emit the corresponding RoadSnapNotice entries (Code, Title, TracePointIndexes) instead of a constant empty list.
Traces: geo-routes.snap-to-roads-not-matched-notice; geo-routes.snap-to-roads-notice-trace-points-not-matched; geo-routes.snap-to-roads-unmatched-point-notice.

Cause 2 of 6: CalculateRoutes reports MainLanguageNotFound for a language it does support.
AWS: Smithy: CalculateRoutesResponse.Notices is @required (RouteResponseNoticeList); RouteResponseNotice carries Code (@required, enum RouteResponseNoticeCode whose values include MainLanguageNotFound) and an optional Impact, and RouteLeg.Language (LanguageTag, @length 2..35) names the language the leg's instructions are in. Recorded behavior: a request with Languages=['es-ES'] and LegAdditionalFeatures=['TravelStepInstructions'] is answered with NO MainLanguageNotFound notice and legs whose Language is the requested tag; the notice is reserved for a requested language whose localization is unavailable, in which case the leg reports the language actually used rather than echoing the unusable tag.
Emulator: answers the es-ES request with Notices [{'Code': 'MainLanguageNotFound', 'Impact': 'Low'}], i.e. it reports the fallback notice for a recognized language.
Why: a client uses this notice to decide whether to show localized instructions or fall back to its own strings; reporting it for a supported language makes every localized request look degraded, and the notice loses all signal value for the case it exists for.
Fix: the CalculateRoutes handler's language-resolution component - resolve the requested Languages against the set the emulator can localize, emit MainLanguageNotFound only when none resolves, and set the leg's Language to the language actually used.
Traces: geo-routes.calculate-routes-unsupported-language-triggers-fallback-notice.

Cause 3 of 6: CalculateRouteMatrix answers unroutable origin/destination pairs as successful cells, so no per-cell Error is emitted and ErrorCount stays 0.
AWS: Smithy: RouteMatrixEntry.Error targets enum RouteMatrixErrorCode (NoMatch, NoMatchDestination, NoMatchOrigin, NoRoute, OutOfBounds, OutOfBoundsDestination, OutOfBoundsOrigin, Other, Violation) and CalculateRouteMatrixResponse.ErrorCount is @required (Integer, @range min 0), the number of entries that failed. Recorded behavior: a car matrix mixing mainland US pairs with a Honolulu origin returns Error 'NoRoute' on each Honolulu cell (no road route crosses the Pacific), no Error on the mainland cells, and ErrorCount equal to the number of cells carrying an Error.
Emulator: returns a cell with fabricated metrics instead - e.g. {'Distance': 4216383, 'Duration': 303337} for Honolulu -> Portland by car - with no Error member, and ErrorCount 0.
Why: a client cannot distinguish a failed cell from a succeeded one: it reads a plausible 4216 km / 84 h car route across open ocean, and the ErrorCount it uses as a cheap 'any failures?' check reports a clean matrix.
Fix: the CalculateRouteMatrix handler's per-cell routing path - when no route exists, emit Error = NoRoute instead of synthesized Distance/Duration, and derive ErrorCount from the cells that carry an Error.
Traces: geo-routes.calculate-route-matrix-no-route-error-count-consistency.

Cause 4 of 6: a cell rejected by a non-directional RoutingBoundary is given a direction-qualified error code.
AWS: Smithy: RouteMatrixErrorCode distinguishes the unqualified OutOfBounds from the qualified OutOfBoundsOrigin/OutOfBoundsDestination; CalculateRouteMatrixRequest.RoutingBoundary targets RouteMatrixBoundary, whose Geometry may be a Circle (a center plus a radius, carrying no origin/destination direction). Recorded behavior: with RoutingBoundary.Geometry.Circle set and a destination outside the circle, the cell's Error is the unqualified 'OutOfBounds', and ErrorCount is 1.
Emulator: sets Error 'OutOfBoundsDestination' on that cell (with Distance 0, Duration 0). Hypothesis: the handler attributes the boundary failure to whichever endpoint it tested last rather than emitting the unqualified code when the boundary cannot attribute the failure to one endpoint.
Why: clients branch on this enum - a qualified code says 'this destination is unusable, drop it' while OutOfBounds says only 'this pair is outside the boundary, widen it'. The emulator's code makes a client discard a destination AWS never blamed.
Fix: the CalculateRouteMatrix handler's per-cell error-classification component.
Traces: geo-routes.calculate-route-matrix-out-of-bounds-destination.

Cause 5 of 6: requested stop/service time is dropped from the totals that aggregate it.
AWS: Smithy: RouteWaypoint.StopDuration (DurationSeconds, @range max 49999, default 0, unit seconds) is the duration of the stop at a CalculateRoutes waypoint, and WaypointOptimizationWaypoint.ServiceDuration (DurationSeconds, default 0, unit seconds) is the 'service time spent at the waypoint' for OptimizeWaypoints, whose response TimeBreakdown (@required) splits the cycle into RestDuration/ServiceDuration/TravelDuration/WaitDuration and whose Duration is @required. Recorded behavior: the stopped time is part of the overall duration - with everything else in the request held identical, adding a stop of N seconds raises CalculateRoutes' Routes[0].Summary.Duration by approximately N, and adding ServiceDuration N raises OptimizeWaypoints' Duration by exactly N (TimeBreakdown.ServiceDuration also reporting N).
Emulator: ignores both in the totals. CalculateRoutes returns Summary.Duration 1288 both with StopDuration 0 and with StopDuration 900 on the same waypoint (delta 0). OptimizeWaypoints returns Duration 113 both with and without ServiceDuration 600 (delta 0), even though TimeBreakdown.ServiceDuration does echo the requested 600 - so the aggregate Duration is computed from travel time alone.
Why: these numbers are what a client schedules against. A route plan whose 15-minute stops and 10-minute service windows vanish from the total under-states every downstream ETA, appointment window and driver-hours check, and the inconsistency between TimeBreakdown.ServiceDuration and Duration makes the response internally contradictory.
Fix: the CalculateRoutes route-summary aggregation component (include waypoint StopDuration in leg/route Duration) and the OptimizeWaypoints duration aggregation component (Duration = TravelDuration + ServiceDuration + WaitDuration + RestDuration).
Traces: geo-routes.calculate-routes-waypoint-stop-duration-increases-total-duration; geo-routes.optimize-waypoints-service-duration-reflected.

Cause 6 of 6: OptimizeWaypoints refuses any request that sets the modelled Clustering member, with a NotImplemented error.
AWS: Smithy: OptimizeWaypointsRequest.Clustering targets WaypointOptimizationClusteringOptions, whose Algorithm is @required (enum WaypointOptimizationClusteringAlgorithm; 'DrivingDistance assigns all the waypoints that are within driving distance of each other into a single cluster', TopologySegment groups waypoints on one topology segment) with DrivingDistanceOptions for the DrivingDistance algorithm; the operation's only modelled client errors are AccessDeniedException (403), ThrottlingException (429) and ValidationException (400) - NotImplemented is not one of them. Recorded behavior: a request with Clustering.Algorithm=DrivingDistance and DrivingDistanceOptions.DrivingDistance=5000 is answered 200 and every entry of OptimizedWaypoints carries a ClusterIndex, with waypoints within the configured driving distance of each other sharing an index and a waypoint well outside it in a different one.
Emulator: answers with error code 'NotImplemented' and the message 'operation OptimizeWaypoints with Clustering is scaffolded but not implemented in this emulator', i.e. the operation is implemented but its Clustering branch is a scaffold stub, and it refuses with an error code that is not in the service's error set.
Why: a caller using clustered waypoint optimization gets an unmodelled error its SDK cannot classify, instead of a clustered sequence; no part of the clustering contract (ClusterIndex on OptimizedWaypoints) is observable.
Fix: the OptimizeWaypoints handler's clustering component - implement the Clustering option (at minimum the DrivingDistance algorithm with DrivingDistanceOptions) and populate ClusterIndex on OptimizedWaypoints.
Traces: geo-routes.optimize-waypoints-driving-distance-clustering-groups-nearby-waypoints.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator (28/62 passed; all 34 failures in geo-routes).

Cause 1 - SnapToRoads notices always empty:
  geo-routes.snap-to-roads-not-matched-notice (3 downtown-Seattle fixes + [-140.0, 40.0] in the open Pacific, index 3, Truck mode): AssertionError: no anomaly notice in []
  geo-routes.snap-to-roads-notice-trace-points-not-matched (ocean point at index 2): AssertionError: expected index 2 (open ocean) flagged, got notices []
  geo-routes.snap-to-roads-unmatched-point-notice (ocean point at index 2): AssertionError: no anomaly notice in []
  Expected in each: a Notices entry with Code TracePointsNotMatched or TracePointsMovedByLargeDistance whose TracePointIndexes contains exactly the ocean point's index.

Cause 2 - geo-routes.calculate-routes-unsupported-language-triggers-fallback-notice:
  AssertionError: a recognized language must not report MainLanguageNotFound, got notices=[{'Code': 'MainLanguageNotFound', 'Impact': 'Low'}]  (request: Languages=['es-ES'], LegAdditionalFeatures=['TravelStepInstructions'])

Cause 3 - geo-routes.calculate-route-matrix-no-route-error-count-consistency:
  Origins=[Seattle, Honolulu], Destinations=[Portland, San Francisco], TravelMode=Car.
  AssertionError: expected Error=NoRoute for a Honolulu-origin car pair, got entry={'Distance': 4216383, 'Duration': 303337}
  Expected: both Honolulu cells carry Error 'NoRoute' and ErrorCount == 2; observed ErrorCount 0 and no Error members.

Cause 4 - geo-routes.calculate-route-matrix-out-of-bounds-destination:
  RoutingBoundary={'Geometry': {'Circle': {'Center': Seattle, 'Radius': 50000}}}, Destinations=[Portland].
  AssertionError: expected RouteMatrixErrorCode OutOfBounds for a Circle boundary ... got entry={'Distance': 0, 'Duration': 0, 'Error': 'OutOfBoundsDestination'}

Cause 5 - dropped stop/service time:
  geo-routes.calculate-routes-waypoint-stop-duration-increases-total-duration: AssertionError: expected the 900s stop to raise total Duration by roughly that much, got delta=0 (with_stop=1288, no_stop=1288) - two identical CalculateRoutes requests differing only in Waypoints[0].StopDuration (0 vs 900).
  geo-routes.optimize-waypoints-service-duration-reflected: AssertionError: expected overall Duration to grow by exactly the added ServiceDuration (600s), got a delta of 0s (serviced=113, baseline=113) - two identical OptimizeWaypoints requests differing only in Waypoints[0].ServiceDuration (absent vs 600); TimeBreakdown.ServiceDuration did echo 600.

Cause 6 - geo-routes.optimize-waypoints-driving-distance-clustering-groups-nearby-waypoints [error]:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the OptimizeWaypoints operation: operation OptimizeWaypoints with Clustering is scaffolded but not implemented in this emulator
  Request: Origin downtown Seattle, Destination Redmond, Waypoints close-a/close-b (~150 m apart in Bellevue) and far (Redmond), Clustering={'Algorithm': 'DrivingDistance', 'DrivingDistanceOptions': {'DrivingDistance': 5000}}. Expected 200 with ClusterIndex on every OptimizedWaypoints entry, close-a == close-b and far distinct.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the behavioral suite simparity-behavioral-aws-v1 (service geo-routes).
2. Cause 1: replay geo-routes.snap-to-roads-not-matched-notice, geo-routes.snap-to-roads-notice-trace-points-not-matched and geo-routes.snap-to-roads-unmatched-point-notice - POST /v2/snap-to-roads with on-road downtown-Seattle fixes plus one point at [-140.0, 40.0] - and inspect the response Notices list and its TracePointIndexes.
3. Cause 2: replay geo-routes.calculate-routes-unsupported-language-triggers-fallback-notice - POST /v2/routes with Languages=['es-ES'] and LegAdditionalFeatures=['TravelStepInstructions'] - and inspect Notices and Routes[0].Legs[0].Language.
4. Cause 3: replay geo-routes.calculate-route-matrix-no-route-error-count-consistency - POST /v2/route-matrix with Origins=[Seattle, Honolulu], Destinations=[Portland, San Francisco], TravelMode=Car - and inspect each cell's Error and the response ErrorCount.
5. Cause 4: replay geo-routes.calculate-route-matrix-out-of-bounds-destination - the same single pair first unbounded (control) then with RoutingBoundary.Geometry.Circle centred on Seattle with Radius 50000 - and inspect RouteMatrix[0][0].Error.
6. Cause 5: replay geo-routes.calculate-routes-waypoint-stop-duration-increases-total-duration (two POST /v2/routes differing only in Waypoints[0].StopDuration 0 vs 900; compare Routes[0].Summary.Duration) and geo-routes.optimize-waypoints-service-duration-reflected (two POST /v2/optimize-waypoints differing only in Waypoints[0].ServiceDuration; compare the top-level Duration and TimeBreakdown).
7. Cause 6: replay geo-routes.optimize-waypoints-driving-distance-clustering-groups-nearby-waypoints - POST /v2/optimize-waypoints with Clustering.Algorithm=DrivingDistance and DrivingDistanceOptions.DrivingDistance=5000 - and inspect OptimizedWaypoints[*].ClusterIndex.
