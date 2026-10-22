Issue-Severity: MEDIUM
Issue-Gap-Class: unimplemented-member

# CalculateRoutes returns no travel steps

_Filed: 2026-10-22T07:00:00Z_
_Operation: CalculateRoutes | Class: unimplemented-member_

## Description

`CalculateRoutes` on `service-com-amazonaws-georoutes-routesservice` computes its routes,
legs, geometry, distances, durations and times from the road model in `src/state.rs`, but
every leg comes back with `TravelSteps` and `AfterTravelSteps` as the empty required list:

- `RouteVehicleLegDetails.TravelSteps` / `AfterTravelSteps` — always `[]`
- `RoutePedestrianLegDetails.TravelSteps` / `AfterTravelSteps` — always `[]`

The real service emits a step per manoeuvre of a leg (`Depart`, `Continue`, `Turn`,
`Keep`, `RoundaboutEnter`, `RoundaboutExit`, `Arrive`, …), each carrying its own distance,
duration and geometry offset, plus the manoeuvre's own details: a turn angle and intensity,
a steering direction, an intersection, a signpost, a relative roundabout exit, and an
`Instruction` string in the requested `Languages` and `InstructionsMeasurementSystem`.

A request that explicitly configures steps is declined rather than answered with the empty
list: `TravelStepType` and `InstructionsMeasurementSystem` answer HTTP 501 today (see the
module header of `src/ops/calculate_routes.rs`). A request that configures neither still
gets a 200 whose step lists are empty, which is the gap this issue records.

## Why it is not a small fix

The step shapes are manoeuvre data, and the road model this service computes over carries
none of it. `src/state.rs` materialises a grid of straight runs — a meridian road every
`ROAD_SPACING_DEG` of longitude and a parallel road every `ROAD_SPACING_DEG` of latitude,
each with an id, a label, a posted speed and the travel modes it allows. There are no
junctions, no turn restrictions, no signposts, no roundabouts and no instruction text, so a
turn angle, a steering direction, an intersection or an instruction cannot be derived from
it: they would have to be invented.

Implementing steps therefore means extending the road model first:

1. Give `RoadSegment` its junctions — where two runs meet, and the angle between them — so
   a step boundary and its turn angle are computed rather than chosen.
2. Derive one step per run of the leg's snapped geometry, with the run's own distance and
   duration (the per-run numbers the cost function already produces) and its
   `GeometryOffset` into the leg geometry.
3. Classify each boundary into the model's `RouteVehicleTravelStepType` /
   `RoutePedestrianTravelStepType` from the angle between the two runs, and fill
   `CurrentRoad` / `NextRoad` from the road labels the network already holds.
4. Only then build `Instruction` text, honouring `Languages` and
   `InstructionsMeasurementSystem`, and lift the 501 on `TravelStepType` (`Default` first,
   `TurnByTurn` once the manoeuvre details exist).

## Evidence

`services/service-com-amazonaws-georoutes-routesservice/src/ops/calculate_routes.rs`
returns `travel_steps: Vec::new()` and `after_travel_steps: Vec::new()` in both the
`RouteVehicleLegDetails` and `RoutePedestrianLegDetails` arms of `leg_output`, with the
reason stated in the module header ("## Travel steps"). The travel-step shapes and their
members are in `src/ops/types/calculate_routes_types.rs`
(`RouteVehicleTravelStep`, `RoutePedestrianTravelStep`, `RouteTurnStepDetails`,
`RouteKeepStepDetails`, `RouteRoundaboutEnterStepDetails`, `RouteSignpost`, `RouteRoad`),
and the road model that carries no manoeuvre data is `RoadSegment` in `src/state.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `geo-routes` client at it.
2. `CalculateRoutes` with `Origin=[-123.1,49.28]`, `Destination=[-123.12,49.26]` and no
   `TravelStepType` — expect 200 with
   `Routes[0].Legs[0].VehicleLegDetails.TravelSteps` non-empty, each step carrying a
   distance, a duration, a `Type` and a `GeometryOffset` (today: an empty list).
3. The same request with `TravelStepType=Default` — expect 200 with basic steps (today:
   HTTP 501 `NotImplemented`, which is the honest answer until step 2 is computed).
