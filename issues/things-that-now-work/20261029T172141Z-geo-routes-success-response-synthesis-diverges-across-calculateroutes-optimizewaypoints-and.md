Issue-Source: trace-sweep-divergence
Issue-Trace: geo-routes-optimize-waypoints-basic-success
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: geo-routes-optimize-waypoints-basic-success geo-routes-optimize-waypoints-duplicate-ids-rejected geo-routes-calculate-routes-truck-travel-mode-options-wiring-success geo-routes-calculate-routes-zero-max-alternatives-returns-single-route geo-routes-optimize-waypoints-origin-only-rejected geo-routes-snap-to-roads-basic-success geo-routes-snap-to-roads-invalid-coordinate-rejected geo-routes-snap-to-roads-snap-radius-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.georoutes#RoutesService
Issue-Key: geo-routes|OptimizeWaypoints|2xx->2xx|$.Connections[*].Distance
Issue-Key: geo-routes|OptimizeWaypoints|2xx->2xx|$.Connections[*].From
Issue-Key: geo-routes|OptimizeWaypoints|2xx->2xx|$.Connections[*].To
Issue-Key: geo-routes|OptimizeWaypoints|2xx->2xx|$.Connections[*].TravelDuration
Issue-Key: geo-routes|OptimizeWaypoints|2xx->2xx|$.Distance
Issue-Key: geo-routes|OptimizeWaypoints|2xx->2xx|$.Duration
Issue-Key: geo-routes|OptimizeWaypoints|2xx->2xx|$.TimeBreakdown.TravelDuration
Issue-Key: geo-routes|OptimizeWaypoints|2xx->2xx|$headers.x-amz-geo-pricing-bucket
Issue-Key: geo-routes|CalculateRoutes|2xx->2xx|$.Routes[*].Legs[*].VehicleLegDetails.Arrival.Place.OriginalPosition[*]
Issue-Key: geo-routes|CalculateRoutes|2xx->2xx|$.Routes[*].Legs[*].VehicleLegDetails.Arrival.Place.Position[*]
Issue-Key: geo-routes|CalculateRoutes|2xx->2xx|$.Routes[*].Legs[*].VehicleLegDetails.Departure.Place.OriginalPosition[*]
Issue-Key: geo-routes|CalculateRoutes|2xx->2xx|$.Routes[*].Legs[*].VehicleLegDetails.Departure.Place.Position[*]
Issue-Key: geo-routes|CalculateRoutes|2xx->2xx|$.Routes[*].MajorRoadLabels[*]
Issue-Key: geo-routes|SnapToRoads|2xx->2xx|$.SnappedGeometry.Polyline
Issue-Key: geo-routes|SnapToRoads|2xx->2xx|$.SnappedTracePoints[*].Confidence
Issue-Key: geo-routes|SnapToRoads|2xx->2xx|$.SnappedTracePoints[*].SnappedPosition[*]
Issue-Key: geo-routes|SnapToRoads|2xx->2xx|$headers.x-amz-geo-pricing-bucket
Issue-Commit: 4fccfcb9e83e9e12ca223d9fc117bac8a0390f59

# Success-response synthesis diverges across CalculateRoutes, OptimizeWaypoints and SnapToRoads, including the pricing-bucket header

> _Note: this issue was identified against the code base at commit `4fccfcb9e83e9e12ca223d9fc117bac8a0390f59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
OptimizeWaypoints

## Description
This issue bundles 5 net-new root causes in the geo-routes success-response synthesis path (CalculateRoutes, OptimizeWaypoints, SnapToRoads). Every step is a 200 whose body/header content diverges. A fixer must close ALL FIVE causes before submitting.

Cause 1 of 5: OptimizeWaypoints does not optimize - it returns the waypoints in request order, so Connections.From/.To name the wrong endpoints.
AWS: Smithy: OptimizeWaypoints -> OptimizeWaypointsResponse.Connections @required (WaypointOptimizationConnection list) where From/To target WaypointId @required and are documented as 'the ID of the starting waypoint in this connection' / 'the ending waypoint'; OptimizedWaypoints is @required and OptimizeSequencingFor selects the objective. Recorded behavior: the recorded answer re-sequences the stops, so the connection chain visits the waypoints in the optimized order, not the order they were submitted in.
Emulator: emits connections that walk the waypoints in submitted order (the From/To of consecutive connections are the request's own sequence), i.e. no sequencing is performed at all.
Why: the whole point of the call is the visiting order; a client that drives its route plan off Connections/OptimizedWaypoints dispatches stops in an unoptimized sequence while believing it was optimized.
Fix: the OptimizeWaypoints handler's sequencing component (it must produce an ordering and build Connections from it), not the serializer.
Traces: geo-routes-optimize-waypoints-basic-success#task#1 (cluster also covers geo-routes-optimize-waypoints-duplicate-ids-rejected#task#2).

Cause 2 of 5: OptimizeWaypoints Distance/Duration/TimeBreakdown are produced by a different travel-metric model.
AWS: Smithy: OptimizeWaypointsResponse.Distance (DistanceMeters, @required) and .Duration (DurationSeconds, @required), WaypointOptimizationConnection.Distance/.TravelDuration (@required), TimeBreakdown @required. Recorded behavior: per-connection distance and travel duration are road-network values consistent with each other, and the totals plus TimeBreakdown.TravelDuration are their sums.
Emulator: returns per-connection distances close to but not equal to the recorded ones and travel durations roughly a third of the recorded values (an implied speed far above the recorded one), and the totals and TimeBreakdown.TravelDuration inherit the same error. The inputs are fixed coordinates in the fixture, so these are reproducible model outputs, not per-run values.
Why: a client scheduling stops from Duration/TimeBreakdown gets ETAs a factor off AWS's, so appointment windows, driver shift checks and rest planning computed on top of them are wrong.
Fix: the OptimizeWaypoints handler's travel-metric component (shared distance/duration estimation).
Traces: geo-routes-optimize-waypoints-basic-success#task#1; geo-routes-optimize-waypoints-origin-only-rejected#task#2.

Cause 3 of 5: the x-amz-geo-pricing-bucket response header reports the wrong bucket for OptimizeWaypoints and SnapToRoads.
AWS: Smithy: OptimizeWaypointsResponse.PricingBucket and SnapToRoadsResponse.PricingBucket are both String, @required, bound with @httpHeader('x-amz-geo-pricing-bucket'). The model does not enumerate the bucket names, so: recorded behavior only - both of these operations are answered with the 'Advanced' bucket; the bucket is a per-operation constant, not a per-request value.
Emulator: answers 'Core' on every OptimizeWaypoints and SnapToRoads response. (The CalculateRouteMatrix fixtures do not flag this header, so the defect is the per-operation bucket mapping, not the header plumbing.)
Why: clients and cost tooling read this header to attribute request cost to a pricing tier; reporting Core for an Advanced-tier operation under-reports spend and makes any emulator-driven cost assertion wrong.
Fix: the per-operation pricing-bucket mapping used by the OptimizeWaypoints and SnapToRoads response builders.
Traces: geo-routes-optimize-waypoints-basic-success#task#1; geo-routes-optimize-waypoints-origin-only-rejected#task#2; geo-routes-snap-to-roads-basic-success#task#1; geo-routes-snap-to-roads-invalid-coordinate-rejected#task#3.

Cause 4 of 5: SnapToRoads does no map matching - snapped positions are the input points, Confidence is a synthetic score, and the snapped polyline is far shorter than the matched geometry.
AWS: Smithy: SnapToRoadsResponse.SnappedTracePoints @required (RoadSnapSnappedTracePoint) where Confidence is a SensitiveDouble @range 0..1 @required documented as 'Confidence value for the correctness of this point match', SnappedPosition @required is the trace point's position snapped onto the road network, and SnappedGeometry carries the snapped polyline in the requested GeometryFormat. Recorded behavior: SnappedPosition differs from the submitted position by a map-matching offset, Confidence varies per point and is LOW (well under 0.2) for points the matcher is unsure about, and the polyline follows the matched road geometry.
Emulator: returns SnappedPosition values that are the submitted coordinates (rounded/re-serialized), Confidence values clustered just under 1.0 (and 1.0) for the very points the recording scores lowest - i.e. a score computed from something other than match quality and never signalling a poor match - and a SnappedGeometry.Polyline with far fewer vertices than the matched geometry.
Why: Confidence is how a client discards bad GPS fixes; a near-1.0 score on a point AWS scored ~0.05 makes the client keep garbage, and un-snapped positions plus a degenerate polyline mean the returned trace is not on the road network at all.
Fix: the SnapToRoads handler's map-matching component (snapping, confidence scoring and snapped-geometry construction).
Traces: geo-routes-snap-to-roads-basic-success#task#1; geo-routes-snap-to-roads-invalid-coordinate-rejected#task#3 (cluster also covers geo-routes-snap-to-roads-snap-radius-out-of-range-rejected#task#2).

Cause 5 of 5: CalculateRoutes leg places lose the request's coordinate precision, are not snapped, and MajorRoadLabels always carries a generated RoadName where a route number is due.
AWS: Smithy: CalculateRoutes leg detail places carry OriginalPosition and Position (both Position = list of Double, @length 2..2); OriginalPosition is the position as supplied in the request and Position is its snapped location, so the two normally differ. RouteMajorRoadLabel has two alternative members, RoadName (LocalizedString) and RouteNumber (RouteNumber, 'Route number of the road'). Recorded behavior: OriginalPosition echoes the submitted coordinate to full precision, Position is a distinct snapped coordinate, and a label for a numbered highway is reported as RouteNumber with the highway designation as its Value.
Emulator: truncates OriginalPosition to fewer decimal places than the request supplied; sets Position to the (truncated) input coordinate re-emitted through float arithmetic, so it carries binary-artefact digits and is not a snapped location; and always emits RoadName, with a synthesized label built from a street name plus a negative numeric suffix, so RouteNumber is never produced.
Why: a client that correlates the echoed OriginalPosition with its own request input finds the coordinate changed; Position is useless as a snapped location; and a client rendering or matching route numbers (signage, highway-based display, alternative-route disambiguation) never sees one and instead gets a label whose numeric suffix is not a real road name.
Fix: the CalculateRoutes handler's place/leg-detail synthesis and its major-road-label component (echo the request coordinate verbatim, snap Position, and classify numbered roads as RouteNumber).
Traces: geo-routes-calculate-routes-truck-travel-mode-options-wiring-success#task#1 (cluster also covers geo-routes-calculate-routes-zero-max-alternatives-returns-single-route#task#1).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (12 fixtures, 19 steps, 0 passed).

Causes 1-3 - geo-routes-optimize-waypoints-basic-success#task#1, expected 200 / actual 200 (cluster affected_count 2, also geo-routes-optimize-waypoints-duplicate-ids-rejected#task#2):
  $.Connections[0].To: expected 'pioneer-square', got 'space-needle'
  $.Connections[1].From: expected 'pioneer-square', got 'space-needle'
  $.Connections[1].To: expected 'space-needle', got 'pioneer-square'   <- submitted order, not optimized order
  $.Connections[0].Distance: expected 1631, got 1568; $.Connections[0].TravelDuration: expected 312, got 113
  $.Connections[1].Distance: expected 3144, got 2861; $.Connections[1].TravelDuration: expected 508, got 206
  $.Distance: expected 4775, got 4429; $.Duration: expected 820, got 319; $.TimeBreakdown.TravelDuration: expected 820, got 319
  $headers.x-amz-geo-pricing-bucket: expected 'Advanced', got 'Core'

Causes 2-3 - geo-routes-optimize-waypoints-origin-only-rejected#task#2, expected 200 / actual 200:
  $.Connections[0].Distance: expected 1227, got 1270; $.Connections[0].TravelDuration: expected 241, got 91
  $.Distance/.Duration/.TimeBreakdown.TravelDuration: expected 1227/241/241, got 1270/91/91
  $headers.x-amz-geo-pricing-bucket: expected 'Advanced', got 'Core'

Causes 3-4 - geo-routes-snap-to-roads-basic-success#task#1, expected 200 / actual 200:
  $.SnappedGeometry.Polyline: expected 'BG0u856C326qpHwHnGgP3NgP3Nof_dkNvM8GnGoL_JkN7L8QvR8Lkc4IoVgKkX8B4D0UkwB8GoQ8G8Q', got 'BGov856Cnv6qpHo4BwM4kC4So4B4S' (far fewer vertices)
  $.SnappedTracePoints[0].Confidence: expected 1, got 0.9750117892464347
  $.SnappedTracePoints[1].Confidence: expected 0.05, got 0.9629349732574718
  $.SnappedTracePoints[2].Confidence: expected 0.04, got 1.0
  $.SnappedTracePoints[3].Confidence: expected 0.12, got 0.9629349732551141
  $.SnappedTracePoints[*].SnappedPosition: expected snapped coords (e.g. -122.33718, 47.61071), got the input coords (e.g. -122.33630000000001, 47.611000000000004)
  $headers.x-amz-geo-pricing-bucket: expected 'Advanced', got 'Core'
  geo-routes-snap-to-roads-invalid-coordinate-rejected#task#3 (cluster affected_count 2, also geo-routes-snap-to-roads-snap-radius-out-of-range-rejected#task#2): same pattern - Confidence expected 0.14 got 0.9750117892464347; Polyline expected 'BG4n856Cj7krpHkIwMoGsJkD4DsJ_JkNjS4cjwBnGrJ' got 'BGov856C_mlrpHo4B_xB'; pricing bucket 'Advanced' vs 'Core'

Cause 5 - geo-routes-calculate-routes-truck-travel-mode-options-wiring-success#task#1, expected 200 / actual 200 (cluster affected_count 2, also geo-routes-calculate-routes-zero-max-alternatives-returns-single-route#task#1):
  $.Routes[0].Legs[0].VehicleLegDetails.Arrival.Place.OriginalPosition[0]: expected -122.3088001, got -122.3088 (precision dropped)
  $.Routes[0].Legs[0].VehicleLegDetails.Arrival.Place.Position[0]: expected -122.3029601, got -122.30900000000001 (input re-emitted, not snapped)
  $.Routes[0].Legs[0].VehicleLegDetails.Departure.Place.Position: expected [-122.3409556, 47.6089359], got [-122.34100000000001, 47.608900000000006]
  $.Routes[0].MajorRoadLabels[1]: expected {'RouteNumber': {'Language': 'en', 'Value': 'WA-518'}}, got {'RoadName': {'Language': 'en', 'Value': 'Meridian Street -244619'}}

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Causes 1-3: replay geo-routes-optimize-waypoints-basic-success step task#1 (POST /v2/optimize-waypoints, Origin + several Waypoints) and compare $.Connections[*].From/.To ordering, $.Connections[*].Distance/.TravelDuration, $.Distance, $.Duration, $.TimeBreakdown.TravelDuration and the x-amz-geo-pricing-bucket response header; repeat with geo-routes-optimize-waypoints-origin-only-rejected step task#2.
3. Causes 3-4: replay geo-routes-snap-to-roads-basic-success step task#1 (POST /v2/snap-to-roads with a GPS trace) and compare $.SnappedTracePoints[*].Confidence and .SnappedPosition against the submitted positions, $.SnappedGeometry.Polyline, and the pricing-bucket header; repeat with geo-routes-snap-to-roads-invalid-coordinate-rejected step task#3.
4. Cause 5: replay geo-routes-calculate-routes-truck-travel-mode-options-wiring-success step task#1 and compare the leg detail Arrival/Departure Place OriginalPosition vs the request coordinates, Place.Position, and $.Routes[0].MajorRoadLabels[*].
