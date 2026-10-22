// SPDX-License-Identifier: CC0-1.0

//! @op CalculateRoutes
//!
//! Computes the route from `Origin` through `Waypoints` to `Destination` over the
//! deterministic road model in [`crate::state`]: one leg per pair of consecutive STOPS
//! (a waypoint with `PassThrough` set does not split the route, per its own
//! documentation), each leg's geometry snapped onto the road network, and its distance
//! and duration costed by the single cost function
//! [`ComAmazonawsGeoroutesRoutesserviceState::compute_route`] that CalculateRouteMatrix
//! and OptimizeWaypoints also use — so a matrix cell and this route cannot disagree.
//!
//! `TravelMode` `Transit` and `Intermodal` answer 501: a faithful answer needs transit
//! lines and timetables, which this emulator holds no data for, and the leg shapes those
//! modes return (`TransitLegDetails`, boarding steps, next departures) cannot be computed
//! from the road model. So do `Exclude` (a country exclusion is "strictly excluded", and
//! no road carries the country it is in), any `SpanAdditionalFeatures`, and any
//! `LegAdditionalFeatures` other than `Summary`: those ask for data the road model does
//! not carry, and an empty list in their place would be absence dressed as success.
//!
//! `Avoid` is best-effort by its own documentation — a violated avoidance "produces a
//! notice for the violation" — so a route that uses a controlled-access highway the
//! request asked to avoid carries the model's `ViolatedAvoidControlledAccessHighway`
//! notice. Nothing else `Avoid` names exists on this network to be used or violated (no
//! toll road, ferry, tunnel, dirt road, car shuttle train, seasonal closure or zone), and
//! the computed path never turns back on itself.
//!
//! ## Travel steps
//!
//! A leg's `TravelSteps` and `AfterTravelSteps` are the empty required lists, and that is
//! the whole of what this model can say about steps: a step is a manoeuvre with an
//! instruction, a turn angle, a signpost, an intersection or a roundabout exit, and the
//! road model in [`crate::state`] is a grid of straight runs that carries none of those —
//! there is no junction geometry to derive a turn from and no text to put in an
//! instruction. The empty list is therefore the answer for a request that asks for no
//! steps, and a request that DOES ask for them — `TravelStepType` or
//! `InstructionsMeasurementSystem` — is declined with 501 rather than answered with it.
//! The gap is filed as its own issue,
//! `issues/things-that-do-not-work/20261022T070000Z-georoutes-calculateroutes-travel-steps-gap.md`,
//! which states what the road model needs before a step can be computed; nothing here
//! fabricates one.
//!
//! `Allow`, `Tolls`, `Traffic`, `TravelModeOptions` and `Languages` have nothing to act on
//! for the same reason: the model carries no HOV/HOT lane, toll, traffic variation,
//! vehicle restriction or localized text.
//!
//! ## Authorization
//!
//! The emulator verifies every request's signature before any handler runs, so this
//! handler checks no credentials. The modelled `Key` (API key) member is consequently not
//! consulted and the modelled `AccessDeniedException` is unreachable here by construction:
//! this emulator holds no API-key records to authorize a key against, and refusing a
//! correctly signed request for want of one would refuse a valid caller. A test must not
//! read acceptance here as a successful key authorization.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CalculateRoutes`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-georoutes-routesservice/smithy-model/com-amazonaws-georoutes-routesservice.json")
//! ```
//!
//! CalculateRoutes computes routes given the following required parameters: Origin and
//! Destination. For more information, see Calculate routes in the Amazon Location Service
//! Developer Guide.
//!
//! ## Input
//!
//! * `Allow` (RouteAllowOptions) — Features that are allowed while calculating a route. Not
//!   supported in ap-southeast-1 and ap-southeast-5 regions for GrabMaps customers.
//! * `ArrivalTime` (TimestampWithTimezoneOffset) — Time of arrival at the destination.
//! * `Avoid` (RouteAvoidanceOptions) — Features that are avoided while calculating a route.
//! * `DepartNow` (SensitiveBoolean) — Uses the current time as the time of departure.
//! * `DepartureTime` (TimestampWithTimezoneOffset) — Time of departure from the origin.
//! * `Destination` (Position, len 2..2) **required** — The final position for the route. In
//!   the World Geodetic System (WGS 84) format: [longitude, latitude].
//! * `DestinationOptions` (RouteDestinationOptions) — Destination related options.
//! * `Driver` (RouteDriverOptions) — Driver related options.
//! * `Exclude` (RouteExclusionOptions) — Features to be strictly excluded while calculating
//!   the route.
//! * `InstructionsMeasurementSystem` (MeasurementSystem, one of: Imperial | Metric) —
//!   Measurement system to be used for instructions within steps in the response.
//! * `Key` (ApiKey, len ..1000) — Optional: The API key to be used for authorization.
//! * `Languages` (LanguageTagList) — List of languages for instructions within steps.
//! * `LegAdditionalFeatures` (RouteLegAdditionalFeatureList, len ..12) — A list of optional
//!   additional parameters such as timezone that can be requested for each result. …
//!   Summary: Returns a summary of the route, including distance and duration. …
//! * `LegGeometryFormat` (GeometryFormat, one of: FlexiblePolyline | Simple) — Specifies the
//!   format of the geometry returned for each leg of the route. FlexiblePolyline: A compact
//!   and precise encoding format for the leg geometry (see the GitHub repository for
//!   https://github.com/aws-geospatial/polyline). Simple: A less compact encoding.
//! * `MaxAlternatives` (Integer) — Maximum number of alternative routes to be provided in the
//!   response, if available.
//! * `OptimizeRoutingFor` (RoutingObjective, one of: FastestRoute | ShortestRoute) — Controls
//!   the trade-off between achieving the shortest travel time (FastestRoute) and achieving
//!   the shortest physical distance ((ShortestRoute). Default value: FastestRoute
//! * `Origin` (Position, len 2..2) **required** — The start position for the route in World
//!   Geodetic System (WGS 84) format: [longitude, latitude].
//! * `OriginOptions` (RouteOriginOptions) — Specifies how the origin point should be matched
//!   to the road network.
//! * `SpanAdditionalFeatures` (RouteSpanAdditionalFeatureList, len ..24) — A list of optional
//!   features such as SpeedLimit that can be requested for a Span.
//! * `Tolls` (RouteTollOptions) — Toll related options.
//! * `Traffic` (RouteTrafficOptions) — Traffic related options.
//! * `TravelMode` (RouteTravelMode, one of: Car | Intermodal | Pedestrian | Scooter | Transit
//!   | Truck) — Specifies the mode of transport when calculating a route. Used in estimating
//!   the speed of travel and road compatibility. Default value: Car
//! * `TravelModeOptions` (RouteTravelModeOptions) — Travel mode related options.
//! * `TravelStepType` (RouteTravelStepType, one of: Default | TurnByTurn) — Type of step
//!   returned by the response.
//! * `Waypoints` (RouteWaypointList) — List of waypoints between the Origin and Destination.
//!   Max length: 23
//!
//! ## Output
//!
//! * `LegGeometryFormat` (GeometryFormat, one of: FlexiblePolyline | Simple) **required** —
//!   Specifies the format of the geometry returned for each leg of the route.
//! * `Notices` (RouteResponseNoticeList) **required** — Notices are additional information
//!   returned that indicate issues that occurred during route calculation.
//! * `PricingBucket` (String) **required** — The pricing bucket for which the query is
//!   charged at.
//! * `Routes` (RouteList) **required** — The path from the origin to the destination.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You don't have sufficient access to perform this
//!   action.
//! * `InternalServerException` (HTTP 500) — The request processing has failed because of an
//!   unknown error, exception or failure.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints specified
//!   by an AWS service.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{self, CachedLeg, Instant, LegType, Position, RoutingObjective, TravelMode};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CalculateRoutesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-georoutes-routesservice.json");

// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.georoutes#CalculateRoutes")]
#[derive(Debug)]
pub enum CalculateRoutesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::calculate_routes::CalculateRoutesError`). The wrapper is kept because
/// `TravelMode` `Transit` and `Intermodal` are answered with its 501.
pub type CalculateRoutesError = crate::scaffolded::Scaffolded<CalculateRoutesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<CalculateRoutesErrorModeled> for CalculateRoutesError {
    fn from(modeled: CalculateRoutesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The model's own `ValidationException` (HTTP 400) for a request member this service
/// refuses, naming the member and the rule it broke.
fn invalid(member: &str, message: String) -> CalculateRoutesError {
    CalculateRoutesErrorModeled::ValidationException(ValidationException {
        message: message.clone(),
        reason: "FieldValidationFailed".to_string(),
        field_list: vec![ValidationExceptionField {
            name: member.to_string(),
            message,
        }],
    })
    .into()
}

/// The model's `InternalServerException` (HTTP 500), for a failure of this emulator
/// rather than of the request.
fn internal(message: &str) -> CalculateRoutesError {
    CalculateRoutesErrorModeled::InternalServerException(InternalServerException {
        message: message.to_string(),
    })
    .into()
}

/// The documented maximum number of waypoints between the origin and the destination
/// ("Max length: 23" on `CalculateRoutesRequest$Waypoints`).
const MAX_WAYPOINTS: usize = 23;

/// The upper bound of `RouteWaypoint$StopDuration`'s modeled range (`0..49999`).
const MAX_STOP_DURATION_S: i64 = 49_999;

/// Where the route's clock is pinned: at its departure or at its arrival.
#[derive(Debug, Clone, Copy)]
enum Anchor {
    Departure(Instant),
    Arrival(Instant),
}

/// What rendering a computed route into the answer needs beyond the route itself.
#[derive(Debug, Clone, Copy)]
struct Rendering<'a> {
    travel_mode: TravelMode,
    geometry_format: &'a str,
    anchor: Anchor,
    /// Whether `LegAdditionalFeatures` asked for the leg summary.
    wants_leg_summary: bool,
    /// Whether the route violates an avoidance the request asked for.
    violated_avoidance: bool,
}

/// One point of the requested sequence: origin, a waypoint, or the destination.
#[derive(Debug, Clone)]
struct SequencePoint {
    position: Position,
    /// Index in the request's `Waypoints` list, for the places that report one.
    waypoint_index: Option<usize>,
    /// Whether the point splits the route into legs. False only for a `PassThrough`
    /// waypoint, which "is passed through and doesn't split the route into different
    /// legs".
    is_stop: bool,
    /// `StopDuration`: how long the traveller stays at the point.
    stop_duration_s: i64,
}

/// One leg of the answer: the computed sub-legs between two stops, merged.
#[derive(Debug, Clone)]
struct MergedLeg {
    distance_m: i64,
    duration_s: i64,
    geometry: Vec<Position>,
    /// The pass-through waypoints inside this leg, with their offset into its geometry.
    pass_through: Vec<(SequencePoint, usize)>,
    start: SequencePoint,
    end: SequencePoint,
}

/// The `CalculateRoutes` operation.
pub struct CalculateRoutes<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CalculateRoutes<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CalculateRoutes<Eng>
{
    type Input = CalculateRoutesInput;
    type Output = CalculateRoutesOutput;
    type Error = CalculateRoutesError;

    const NAME: &'static str = "CalculateRoutes";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CALCULATE_ROUTES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CALCULATE_ROUTES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;

        let travel_mode = match input.travel_mode.as_deref() {
            // `Default value: Car`.
            None => TravelMode::Car,
            Some(value) => TravelMode::from_model_values(value, ROUTE_TRAVEL_MODE_VALUES)
                .ok_or_else(|| invalid("TravelMode", format!("TravelMode {value} is not a RouteTravelMode")))?,
        };
        if matches!(travel_mode, TravelMode::Transit | TravelMode::Intermodal) {
            // No transit network or timetable is held, so there is nothing to compute a
            // transit leg from. 501 says "not written yet", which is the truth.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "CalculateRoutes with TravelMode Transit or Intermodal",
            ));
        }
        let objective = match input.optimize_routing_for.as_deref() {
            // `Default value: FastestRoute`.
            None => RoutingObjective::FastestRoute,
            Some(value) => RoutingObjective::from_model(value).ok_or_else(|| {
                invalid(
                    "OptimizeRoutingFor",
                    format!("OptimizeRoutingFor {value} is not a RoutingObjective"),
                )
            })?,
        };
        let geometry_format = state::geometry_format("LegGeometryFormat", input.leg_geometry_format.as_ref())
            .map_err(|message| invalid("LegGeometryFormat", message))?;

        // What this slice does not compute is declined, not answered with an empty list.
        // `Exclude` is "strictly excluded", and nothing in the road model carries the
        // country a road is in, so an exclusion cannot be honoured or reported.
        if input.exclude.is_some() {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "CalculateRoutes with Exclude",
            ));
        }
        // An avoidance area is a routing constraint: the route would have to be computed
        // around it, which this model does not do, so it is declined rather than violated
        // silently.
        if input
            .avoid
            .as_ref()
            .is_some_and(|avoid| avoid.areas.as_deref().is_some_and(|areas| !areas.is_empty()))
        {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "CalculateRoutes with Avoid.Areas",
            ));
        }
        // A span is "a section of a Leg for which the requested features have the same
        // values"; this slice computes none of those features, so a request asking for
        // them is declined rather than answered with an empty `Spans` list.
        if input
            .span_additional_features
            .as_deref()
            .is_some_and(|features| !features.is_empty())
        {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "CalculateRoutes with SpanAdditionalFeatures",
            ));
        }
        // The steps of a leg are the turn-by-turn instructions of a route, which this
        // model has no instruction text or manoeuvre data to build. A request that asks
        // for a step type or an instruction measurement system is therefore declined
        // rather than answered with an empty step list.
        if input.travel_step_type.is_some() {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "CalculateRoutes with TravelStepType",
            ));
        }
        if input.instructions_measurement_system.is_some() {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "CalculateRoutes with InstructionsMeasurementSystem",
            ));
        }
        // `Summary` is the one leg feature this slice computes; every other one asks for
        // data the road model does not carry (tolls, incidents, elevation, step
        // instructions, zones, truck road types, transit stops and departures).
        let leg_features = input.leg_additional_features.as_deref().unwrap_or_default();
        if let Some(feature) = leg_features.iter().find(|feature| *feature != "Summary") {
            return Err(crate::scaffolded::Scaffolded::not_implemented(&format!(
                "CalculateRoutes with the LegAdditionalFeature {feature}"
            )));
        }
        let wants_leg_summary = leg_features.iter().any(|feature| feature == "Summary");

        // The model pins `MaxAlternatives` to `0..5` and the boundary validator refuses
        // anything outside that before this handler runs. A value that nonetheless reaches
        // here out of range is REFUSED rather than quietly adjusted: answering 200 with a
        // different number of routes than was asked for would let a caller read an invalid
        // request as a served one.
        let alternatives = input.max_alternatives.unwrap_or(0);
        if !(0..=state::MAX_ALTERNATIVES).contains(&alternatives) {
            return Err(invalid(
                "MaxAlternatives",
                format!(
                    "MaxAlternatives must be in [0, {}]; got {alternatives}",
                    state::MAX_ALTERNATIVES
                ),
            ));
        }

        // Documented on `Waypoints`: "Max length: 23". The member carries no modeled
        // length constraint, so the cap is the handler's to apply, and it is what bounds
        // the legs, the geometry and the label aggregation of one request.
        let submitted_waypoints = input.waypoints.as_deref().unwrap_or_default();
        if submitted_waypoints.len() > MAX_WAYPOINTS {
            return Err(invalid(
                "Waypoints",
                format!(
                    "at most {MAX_WAYPOINTS} waypoints are allowed between the origin and the destination; got {}",
                    submitted_waypoints.len()
                ),
            ));
        }

        let anchor = self.anchor(&input)?;

        // The requested sequence: origin, every waypoint, destination.
        let mut sequence = vec![SequencePoint {
            position: state::validate_position("Origin", &input.origin)
                .map_err(|message| invalid("Origin", message))?,
            waypoint_index: None,
            is_stop: true,
            stop_duration_s: 0,
        }];
        for (index, waypoint) in submitted_waypoints.iter().enumerate() {
            let member = format!("Waypoints[{index}].Position");
            // `StopDuration`'s modeled range is `0..49999`, which the boundary validator
            // does not enforce on a defaulted member, so the handler does: the value is
            // added to the route's clock, which must not be driven anywhere by it.
            if !(0..=MAX_STOP_DURATION_S).contains(&waypoint.stop_duration) {
                let member = format!("Waypoints[{index}].StopDuration");
                return Err(invalid(
                    &member,
                    format!(
                        "StopDuration must be in [0, {MAX_STOP_DURATION_S}] seconds; got {}",
                        waypoint.stop_duration
                    ),
                ));
            }
            sequence.push(SequencePoint {
                position: state::validate_position(&member, &waypoint.position)
                    .map_err(|message| invalid(&member, message))?,
                waypoint_index: Some(index),
                is_stop: waypoint.pass_through != Some(true),
                stop_duration_s: waypoint.stop_duration,
            });
        }
        sequence.push(SequencePoint {
            position: state::validate_position("Destination", &input.destination)
                .map_err(|message| invalid("Destination", message))?,
            waypoint_index: None,
            is_stop: true,
            stop_duration_s: 0,
        });

        let positions: Vec<Position> = sequence.iter().map(|point| point.position).collect();

        let state_handle =
            state::service_state(&*self.engine).ok_or_else(|| internal("the routes service state is unavailable"))?;

        // Every route of the answer is computed under ONE lock on the shared state: at
        // most `MAX_ALTERNATIVES + 1` routes, and the lock is taken once rather than per
        // route.
        let (computed_routes, pricing_bucket) = {
            let mut state = state_handle.lock();
            let computed: Vec<state::CachedRoute> = (0..=u32::try_from(alternatives).unwrap_or(0))
                .map(|variant| state.compute_route(&positions, travel_mode, objective, variant))
                .collect();
            (computed, state.pricing_bucket.clone())
        };

        let mut routes = Vec::new();
        for computed in &computed_routes {
            let legs = merge_legs(&sequence, &computed.legs);
            // "Avoidance is on a best-case basis. If an avoidance can't be satisfied for a
            // particular case, it violates the avoidance and the returned response
            // produces a notice for the violation." The only avoidance this road model can
            // violate is of controlled-access highways: it carries no toll road, ferry,
            // tunnel, dirt road, car shuttle train, seasonal closure or zone to use, and
            // the computed path never turns back on itself.
            let violated_avoidance = input
                .avoid
                .as_ref()
                .is_some_and(|avoid| avoid.controlled_access_highways == Some(true))
                && computed.uses_controlled_access_highway;
            routes.push(self.route_output(
                &legs,
                &computed.major_road_labels,
                Rendering {
                    travel_mode,
                    geometry_format: &geometry_format,
                    anchor,
                    wants_leg_summary,
                    violated_avoidance,
                },
            )?);
        }

        Ok(CalculateRoutesOutput {
            leg_geometry_format: geometry_format,
            // Nothing about a route over this road model is worth a notice: no language
            // was dropped, and no transit data was consulted.
            notices: Vec::new(),
            pricing_bucket,
            routes,
        })
    }
}

impl<Eng> CalculateRoutes<Eng> {
    /// Where the route's clock is pinned. `DepartureTime`, `ArrivalTime` and `DepartNow`
    /// each pin it, so naming more than one is contradictory and refused; naming none
    /// departs now, which is what the service does for a request that gives no time.
    fn anchor(&self, input: &CalculateRoutesInput) -> Result<Anchor, CalculateRoutesError> {
        let depart_now = input.depart_now == Some(true);
        let named = usize::from(input.departure_time.is_some())
            + usize::from(input.arrival_time.is_some())
            + usize::from(depart_now);
        if named > 1 {
            return Err(invalid(
                "DepartureTime",
                "DepartureTime, ArrivalTime and DepartNow each set the time of the route, so at most one may be supplied"
                    .to_string(),
            ));
        }
        if let Some(departure) = &input.departure_time {
            return Ok(Anchor::Departure(
                state::parse_timestamp("DepartureTime", departure)
                    .map_err(|message| invalid("DepartureTime", message))?,
            ));
        }
        if let Some(arrival) = &input.arrival_time {
            return Ok(Anchor::Arrival(
                state::parse_timestamp("ArrivalTime", arrival).map_err(|message| invalid("ArrivalTime", message))?,
            ));
        }
        Ok(Anchor::Departure(state::now()))
    }

    /// One `Route` of the answer, with its legs, labels and summary.
    fn route_output(
        &self,
        legs: &[MergedLeg],
        major_road_labels: &[String],
        rendering: Rendering<'_>,
    ) -> Result<Route, CalculateRoutesError> {
        let distance: i64 = state::clamp_cost(legs.iter().map(|leg| leg.distance_m as f64).sum());
        let duration: i64 = state::clamp_cost(legs.iter().map(|leg| leg.duration_s as f64).sum());

        // The clock walks the legs forward from the departure. An `ArrivalTime` request
        // pins the far end instead, so the departure is the arrival less everything the
        // route spends: travel plus the stop at each intermediate waypoint. Every step is
        // saturating, so no request's numbers can drive the clock past what an `i64` of
        // seconds holds (the model caps each `StopDuration` at 49,999 and each duration at
        // 4,294,967,295, and the waypoints at 23, but the arithmetic does not rely on it).
        let stops: i64 = legs
            .iter()
            .take(legs.len().saturating_sub(1))
            .map(|leg| leg.end.stop_duration_s)
            .fold(0_i64, i64::saturating_add);
        let mut clock = match rendering.anchor {
            Anchor::Departure(departure) => departure,
            Anchor::Arrival(arrival) => {
                state::shift_seconds(arrival, duration.saturating_add(stops).saturating_neg())
            }
        };

        let mut out = Vec::with_capacity(legs.len());
        for leg in legs {
            let departure = clock;
            let arrival = state::shift_seconds(departure, leg.duration_s);
            clock = state::shift_seconds(arrival, leg.end.stop_duration_s);
            out.push(self.leg_output(leg, rendering, departure, arrival)?);
        }

        Ok(Route {
            legs: out,
            major_road_labels: major_road_labels
                .iter()
                .map(|label| RouteMajorRoadLabel {
                    road_name: Some(LocalizedString {
                        language: None,
                        value: label.clone(),
                    }),
                    route_number: None,
                })
                .collect(),
            summary: Some(RouteSummary {
                distance,
                duration,
                tolls: None,
            }),
        })
    }

    /// One `RouteLeg`: its geometry in the requested format, and the details of the type
    /// the travel mode produces.
    fn leg_output(
        &self,
        leg: &MergedLeg,
        rendering: Rendering<'_>,
        departure: Instant,
        arrival: Instant,
    ) -> Result<RouteLeg, CalculateRoutesError> {
        let travel_mode = rendering.travel_mode;
        let wants_leg_summary = rendering.wants_leg_summary;
        let violated_avoidance = rendering.violated_avoidance;
        let geometry = leg_geometry(&leg.geometry, rendering.geometry_format)?;
        let leg_type = LegType::for_mode(travel_mode);
        let departure_time =
            state::format_timestamp(departure).ok_or_else(|| internal("the leg departure time is not representable"))?;
        let arrival_time =
            state::format_timestamp(arrival).ok_or_else(|| internal("the leg arrival time is not representable"))?;

        let mut route_leg = RouteLeg {
            geometry,
            travel_mode: travel_mode.as_model().to_string(),
            r#type: leg_type.as_model().to_string(),
            ..Default::default()
        };

        match leg_type {
            LegType::Pedestrian => {
                route_leg.pedestrian_leg_details = Some(RoutePedestrianLegDetails {
                    after_travel_steps: Vec::new(),
                    arrival: RoutePedestrianArrival {
                        place: pedestrian_place(&leg.end),
                        time: Some(arrival_time),
                    },
                    departure: RoutePedestrianDeparture {
                        place: pedestrian_place(&leg.start),
                        time: Some(departure_time),
                    },
                    notices: Vec::new(),
                    pass_through_waypoints: pass_through_waypoints(leg),
                    // Spans are only computed for the features `SpanAdditionalFeatures`
                    // asks for, and this slice computes none of them. Steps are empty for
                    // the reason the module header gives: this road model carries no
                    // junction, signpost or instruction data a step could be derived from.
                    spans: Vec::new(),
                    summary: wants_leg_summary.then_some(RoutePedestrianSummary {
                        overview: Some(RoutePedestrianOverviewSummary {
                            distance: leg.distance_m,
                            duration: leg.duration_s,
                        }),
                        travel_only: Some(RoutePedestrianTravelOnlySummary {
                            duration: leg.duration_s,
                        }),
                    }),
                    travel_steps: Vec::new(),
                });
            }
            LegType::Vehicle => {
                route_leg.vehicle_leg_details = Some(RouteVehicleLegDetails {
                    after_travel_steps: Vec::new(),
                    arrival: RouteVehicleArrival {
                        place: vehicle_place(&leg.end),
                        time: Some(arrival_time),
                    },
                    departure: RouteVehicleDeparture {
                        place: vehicle_place(&leg.start),
                        time: Some(departure_time),
                    },
                    incidents: Vec::new(),
                    notices: if violated_avoidance {
                        // The route uses a controlled-access highway the request asked to
                        // avoid, which the model's own notice code names.
                        vec![RouteVehicleNotice {
                            code: "ViolatedAvoidControlledAccessHighway".to_string(),
                            details: Vec::new(),
                            impact: Some("High".to_string()),
                        }]
                    } else {
                        Vec::new()
                    },
                    pass_through_waypoints: pass_through_waypoints(leg),
                    spans: Vec::new(),
                    summary: wants_leg_summary.then_some(RouteVehicleSummary {
                        overview: Some(RouteVehicleOverviewSummary {
                            // This road model carries no traffic variation, so the best
                            // case and the typical case are the computed duration.
                            best_case_duration: leg.duration_s,
                            distance: leg.distance_m,
                            duration: leg.duration_s,
                            typical_duration: leg.duration_s,
                        }),
                        travel_only: Some(RouteVehicleTravelOnlySummary {
                            best_case_duration: leg.duration_s,
                            duration: leg.duration_s,
                            typical_duration: leg.duration_s,
                        }),
                    }),
                    tolls: Vec::new(),
                    toll_systems: Vec::new(),
                    travel_steps: Vec::new(),
                    truck_road_types: Vec::new(),
                    zones: Vec::new(),
                });
            }
        }
        Ok(route_leg)
    }
}

/// The computed sub-legs grouped into the legs of the answer: a new leg starts at every
/// stop, so `N` waypoints that are stops give `N + 1` legs and a `PassThrough` waypoint
/// gives none of its own.
///
/// `compute_route` returns one sub-leg per consecutive pair of the sequence it was given,
/// so there is always a pair of sequence points for each one; a sub-leg without them is
/// skipped rather than indexed for.
fn merge_legs(sequence: &[SequencePoint], computed: &[CachedLeg]) -> Vec<MergedLeg> {
    let mut legs: Vec<MergedLeg> = Vec::new();
    let mut current: Option<MergedLeg> = None;
    for (index, leg) in computed.iter().enumerate() {
        let (Some(start), Some(end)) = (sequence.get(index).cloned(), sequence.get(index + 1).cloned()) else {
            continue;
        };
        match current.as_mut() {
            None => {
                current = Some(MergedLeg {
                    distance_m: leg.distance_m,
                    duration_s: leg.duration_s,
                    geometry: leg.geometry.clone(),
                    pass_through: Vec::new(),
                    start,
                    end,
                });
            }
            Some(open) => {
                // `start` is the pass-through waypoint this sub-leg continues from: it
                // sits at the end of the geometry accumulated so far.
                let offset = open.geometry.len().saturating_sub(1);
                open.pass_through.push((start, offset));
                open.distance_m = state::clamp_cost(open.distance_m.saturating_add(leg.distance_m) as f64);
                open.duration_s = state::clamp_cost(open.duration_s.saturating_add(leg.duration_s) as f64);
                open.geometry.extend(leg.geometry.iter().skip(1).copied());
                open.end = end;
            }
        }
        if current.as_ref().is_some_and(|open| open.end.is_stop) {
            legs.extend(current.take());
        }
    }
    legs.extend(current);
    legs
}

/// A leg's geometry in the requested format: a Flexible Polyline string, or the plain
/// `LineString` the `Simple` format asks for.
fn leg_geometry(positions: &[Position], format: &str) -> Result<RouteLegGeometry, CalculateRoutesError> {
    if format == "Simple" {
        return Ok(RouteLegGeometry {
            line_string: Some(state::line_string(positions)),
            polyline: None,
        });
    }
    Ok(RouteLegGeometry {
        line_string: None,
        polyline: Some(
            state::encode_flexible_polyline(positions)
                .ok_or_else(|| internal("the leg geometry could not be encoded as a Flexible Polyline"))?,
        ),
    })
}

/// The place of a vehicle leg's departure or arrival.
fn vehicle_place(point: &SequencePoint) -> RouteVehiclePlace {
    RouteVehiclePlace {
        position: state::position_to_vec(point.position),
        original_position: Some(state::position_to_vec(point.position)),
        waypoint_index: point.waypoint_index.and_then(|index| i32::try_from(index).ok()),
        ..Default::default()
    }
}

/// The place of a pedestrian leg's departure or arrival.
fn pedestrian_place(point: &SequencePoint) -> RoutePedestrianPlace {
    RoutePedestrianPlace {
        position: state::position_to_vec(point.position),
        original_position: Some(state::position_to_vec(point.position)),
        waypoint_index: point.waypoint_index.and_then(|index| i32::try_from(index).ok()),
        ..Default::default()
    }
}

/// The pass-through waypoints of a leg, each with its offset into the leg's geometry.
fn pass_through_waypoints(leg: &MergedLeg) -> Vec<RoutePassThroughWaypoint> {
    leg.pass_through
        .iter()
        .map(|(point, offset)| RoutePassThroughWaypoint {
            geometry_offset: i32::try_from(*offset).ok(),
            place: RoutePassThroughPlace {
                position: state::position_to_vec(point.position),
                original_position: Some(state::position_to_vec(point.position)),
                waypoint_index: point.waypoint_index.and_then(|index| i32::try_from(index).ok()),
            },
        })
        .collect()
}
