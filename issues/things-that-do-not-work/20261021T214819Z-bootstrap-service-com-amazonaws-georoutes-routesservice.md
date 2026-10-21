Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the georoutes-routesservice emulator

_Filed: 2026-10-21T21:48:19Z_
_Operation: service-com-amazonaws-georoutes-routesservice | Class: stub_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-georoutes-routesservice/src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 for all 5 operations. Nothing works end-to-end.

This service (restJson1, sigv4 name `geo-routes`) is a *computation* service, not a CRUD service: there is no create/read/delete lifecycle. Its happy path is "compute a route, then refine/extend that computation". Implement this FIRST SLICE of 4 operations so a realistic client can plan a trip:

1. **CalculateRoutes** — `POST /v2/routes`. Required input `Origin` ([lon,lat], len 2..2) and `Destination`; optional `Waypoints`, `TravelMode` (Car|Truck|Pedestrian|Scooter|Transit|Intermodal, default Car), `LegGeometryFormat` (FlexiblePolyline|Simple), `OptimizeRoutingFor` (FastestRoute|ShortestRoute), `DepartureTime`/`ArrivalTime`/`DepartNow`, `MaxAlternatives`, `LegAdditionalFeatures`. Output must populate `Routes` (list<Route>, each with required `Legs` and `MajorRoadLabels`, plus `Summary` carrying Distance/Duration), `LegGeometryFormat` echoing the effective format, `Notices` (may be empty list) and the required `PricingBucket` header (`x-amz-geo-pricing-bucket`). Each `RouteLeg` must carry required `Geometry`, `TravelMode`, `Type`, and `VehicleLegDetails`/`PedestrianLegDetails` matching the leg type. Legs MUST be derived from the actual origin → waypoints → destination sequence (N waypoints ⇒ N+1 legs) and distance/duration MUST be computed geodesically (haversine over the positions) divided by the per-travel-mode speed held in state — never a constant.

2. **CalculateRouteMatrix** — `POST /v2/route-matrix`. Required `Origins` (list<RouteMatrixOrigin>) and `Destinations`; optional `RoutingBoundary`, `TravelMode`, `OptimizeRoutingFor`, `DepartureTime`. Output `RouteMatrix` must be a list of rows of length `Origins.len()`, each row of length `Destinations.len()`, each `RouteMatrixEntry` carrying required `Distance` and `Duration` computed by the SAME cost function CalculateRoutes uses (so matrix[i][j] agrees with a direct CalculateRoutes for that pair), `Error` set for unroutable pairs, `ErrorCount` equal to the number of entries with an `Error`, plus required `RoutingBoundary` and `PricingBucket`.

3. **SnapToRoads** — `POST /v2/snap-to-roads`. Required `TracePoints` (list<RoadSnapTracePoint>) and `SnapRadius` (default 0); optional `SnappedGeometryFormat`, `TravelMode`. Output `SnappedTracePoints` must have ONE entry per input trace point, at the same index, each with required `OriginalPosition` (exactly the submitted position), `SnappedPosition` (the position projected onto the synthetic road network held in state, within `SnapRadius`) and `Confidence` in 0..1 derived from the snap offset; plus `SnappedGeometry`, `SnappedGeometryFormat`, `Notices`, `PricingBucket`.

4. **OptimizeWaypoints** — `POST /v2/optimize-waypoints`. Required `Origin`; optional `Destination`, `Waypoints` (each with `Id`), `OptimizeSequencingFor`, `DepartureTime`, `Driver`. Output `OptimizedWaypoints` must be a permutation of the submitted waypoint ids (origin first, destination last when given) ordered by the reordering actually computed from the positions, each entry carrying required `Id`, `Position`, `DepartureTime` (and `ArrivalTime`) advanced by the computed leg durations; plus `Connections` (one per consecutive pair, with its own distance/duration), required `Distance` and `Duration` equal to the sum over connections, `ImpedingWaypoints`, `TimeBreakdown`, `PricingBucket`.

NEVER STUBS: all four must compute their response from the shared engine state and the request's actual geometry. Hardcoded distances, zero-filled summaries, echoed inputs, or a route whose leg count ignores `Waypoints` are failures. Required output members must all be present (the model marks them `@required`), including the `PricingBucket` HTTP header. Invalid input the model does not already reject — e.g. positions outside lon −180..180 / lat −90..90, both `DepartureTime` and `ArrivalTime` supplied, empty `Origins`/`Destinations`, `MaxAlternatives` < 0 — must return a 400 `ValidationException` (`Scaffolded::validation(..)`), not 501 and not a 200.

## State Model

In `src/state.rs`, `ComAmazonawsGeoroutesRoutesserviceState` holds the deterministic routing model shared by every handler (reached via `crate::state::service_state(&*self.engine)`):

- `road_network: Vec<RoadSegment>` where `RoadSegment { id: String, polyline: Vec<[f64;2]> /* lon,lat */, speed_kph: f64, road_label: String, allows: Vec<TravelMode> }` — the synthetic graph used for snapping and for `MajorRoadLabels`; seeded deterministically (e.g. a lat/lon grid) so repeated calls give identical answers.
- `travel_mode_speeds: HashMap<TravelMode, f64>` (metres/second, e.g. Car 13.9, Truck 11.1, Scooter 8.3, Pedestrian 1.4) — the single cost function used by CalculateRoutes, CalculateRouteMatrix and OptimizeWaypoints so their numbers agree.
- `routing_objective_factors: HashMap<RoutingObjective, f64>` — multipliers applied for FastestRoute vs ShortestRoute.
- `pricing_bucket: String` (e.g. "RoutesRequest") returned in the `x-amz-geo-pricing-bucket` header, and `request_counts: HashMap<String /*operation*/, u64>` so request accounting is real state rather than a literal.
- `calculated_routes: HashMap<String /*deterministic key of (origin,destination,waypoints,travel mode,objective)*/, CachedRoute { legs: Vec<CachedLeg>, distance_m: u64, duration_s: u64, major_road_labels: Vec<String> }>` where `CachedLeg { start: [f64;2], end: [f64;2], distance_m: u64, duration_s: u64, geometry: Vec<[f64;2]>, travel_mode: TravelMode, leg_type: RouteLegType }` — a memo so the same request twice yields byte-identical output and so a matrix cell and a direct route for the same pair cannot disagree.

CalculateIsolines (`POST /v2/isolines`) stays stubbed (501) in this slice and will be filed as its own gap issue.

## Evidence

OPERATIONS.md at the crate root lists 5 operations (CalculateIsolines, CalculateRouteMatrix, CalculateRoutes, OptimizeWaypoints, SnapToRoads) with their routes, required input members (`Origin`/`Destination` on CalculateRoutes, `Origins`/`Destinations` on CalculateRouteMatrix, `TracePoints`+`SnapRadius` on SnapToRoads, `Origin` on OptimizeWaypoints) and required output members (`Routes`, `Notices`, `LegGeometryFormat`, `PricingBucket`; `RouteMatrix`, `ErrorCount`, `RoutingBoundary`; `SnappedTracePoints`, `SnappedGeometryFormat`; `OptimizedWaypoints`, `Connections`, `Distance`, `Duration`, `TimeBreakdown`). Smithy shapes Route/RouteLeg/RouteMatrixEntry/RoadSnapSnappedTracePoint/WaypointOptimizationOptimizedWaypoint confirm the required nested members cited above (e.g. RoadSnapSnappedTracePoint requires OriginalPosition, SnappedPosition and Confidence range 0..1; RouteMatrixEntry requires Distance and Duration). `src/ops/calculate_routes.rs:4` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and line 254 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CalculateRoutes"))`; the other four op files are identical scaffolds, so every operation answers 501 and no tests assert any behaviour.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `geo-routes` client at it.
2. CalculateRoutes with Origin=[-123.1,49.28], Destination=[-123.12,49.26], Waypoints=[{Position:[-123.11,49.27]}], TravelMode=Car — expect 200 with Routes[0].Legs of length 2, non-zero Summary.Distance/Duration, LegGeometryFormat echoed and the x-amz-geo-pricing-bucket header present (today: HTTP 501 NotImplemented).
3. Repeat the identical CalculateRoutes call — expect the same distance/duration/geometry back (deterministic, memoized), not fresh random values.
4. CalculateRouteMatrix with Origins=[[-123.1,49.28],[-123.2,49.3]], Destinations=[[-123.12,49.26]] — expect a 2x1 RouteMatrix whose entry [0][0] Distance/Duration match the direct CalculateRoutes for that pair, and ErrorCount=0.
5. SnapToRoads with 3 TracePoints and SnapRadius=50 — expect 3 SnappedTracePoints in input order, each OriginalPosition equal to the submitted position, SnappedPosition within the radius and Confidence in 0..1.
6. OptimizeWaypoints with Origin, Destination and 3 Waypoints given out of order — expect OptimizedWaypoints to be a permutation of exactly those ids with increasing DepartureTime, Connections one per consecutive pair, and top-level Distance/Duration equal to the sum over Connections.
7. CalculateRoutes with Origin=[-999,99] — expect HTTP 400 ValidationException, not 501 and not 200.
