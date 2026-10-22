// SPDX-License-Identifier: CC0-1.0

//! @op OptimizeWaypoints
//!
//! Orders the submitted waypoints by the cost the shared cost function in
//! [`crate::state`] gives each hop — duration for `FastestRoute`, distance for
//! `ShortestRoute` — taking the cheapest unvisited waypoint at each step, with the origin
//! first and the destination, when one is given, last. The answer carries one connection
//! per consecutive pair of the computed sequence, the clock walked forward across them
//! (travel, then the rest a `Driver.RestCycles` schedule owes, then any wait for an
//! `AppointmentTime`, then the waypoint's `ServiceDuration`), and the totals summed over
//! those connections.
//!
//! `Before` is HONOURED where it can be: a waypoint another unvisited waypoint requires
//! to come after it is not eligible for the next step, so the computed sequence satisfies
//! the constraint whenever the constraints are satisfiable at all. Constraints that name a
//! cycle cannot all hold; the sequence then falls back to cost alone and every waypoint
//! whose `Before` the order breaks comes back as an `ImpedingWaypoints` entry naming that
//! failed constraint — the model's own channel for "waypoints that caused the optimization
//! problem to fail, and the constraints that were unsatisfied".
//!
//! `Clustering`, `Driver.RestProfile`, `Exclude`, `AccessHours`, `Avoid.Areas` and
//! `Avoid.ControlledAccessHighways` answer 501: clustering and the EU rest profile change
//! the sequencing itself (the profile stands for statutory driving and break limits this
//! emulator holds no data for), a country exclusion is "strictly excluded" and no road of
//! the model carries the country it is in, an access window would move the arrival, the
//! wait and the impeding waypoints, and a violated avoidance would have to be reported as
//! a notice this answer has nowhere to carry. The rest of `Avoid` is satisfied trivially
//! (no toll road, ferry, tunnel, dirt road or car shuttle train exists on this network),
//! and `Traffic` and `TravelModeOptions` have nothing to act on: the model carries no
//! traffic variation and no vehicle restriction.
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
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `OptimizeWaypoints`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-georoutes-routesservice/smithy-model/com-amazonaws-georoutes-routesservice.json")
//! ```
//!
//! OptimizeWaypoints calculates the optimal order to travel between a set of waypoints to
//! minimize either the travel time or the distance travelled during the journey, based on
//! road network restrictions and the traffic pattern data. For more information, see
//! Optimize waypoints in the Amazon Location Service Developer Guide.
//!
//! ## Input
//!
//! * `Avoid` (WaypointOptimizationAvoidanceOptions) — Features that are avoided. Avoidance is
//!   on a best-case basis. If an avoidance can't be satisfied for a particular case, this
//!   setting is ignored.
//! * `Clustering` (WaypointOptimizationClusteringOptions) — Clustering allows you to specify
//!   how nearby waypoints can be clustered to improve the optimized sequence.
//! * `DepartureTime` (TimestampWithTimezoneOffset, pattern
//!   ^([1-2][0-9]{3})-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):([0-5][0-9]):([0-5][0-9]|60)(\.[0-9]{0,9})?
//!   …) — Departure time from the waypoint. Time format:YYYY-MM-DDThh:mm:ss.sssZ |
//!   YYYY-MM-DDThh:mm:ss.sss+hh:mm Examples: 2020-04-22T17:57:24Z 2020-04-22T17:57:24+02:00
//! * `Destination` (Position, len 2..2) — The final position for the route in the World
//!   Geodetic System (WGS 84) format: [longitude, latitude].
//! * `DestinationOptions` (WaypointOptimizationDestinationOptions) — Destination related
//!   options.
//! * `Driver` (WaypointOptimizationDriverOptions) — Driver related options.
//! * `Exclude` (WaypointOptimizationExclusionOptions) — Features to be strictly excluded
//!   while calculating the route.
//! * `Key` (ApiKey, len ..1000) — Optional: The API key to be used for authorization. Either
//!   an API key or valid SigV4 signature must be provided when making a request.
//! * `OptimizeSequencingFor` (WaypointOptimizationSequencingObjective, one of: FastestRoute |
//!   ShortestRoute) — Specifies the optimization criteria for the calculated sequence.
//!   Default value: FastestRoute.
//! * `Origin` (Position, len 2..2) **required** — The start position for the route in World
//!   Geodetic System (WGS 84) format: [longitude, latitude].
//! * `OriginOptions` (WaypointOptimizationOriginOptions) — Origin related options.
//! * `Traffic` (WaypointOptimizationTrafficOptions) — Traffic-related options.
//! * `TravelMode` (WaypointOptimizationTravelMode, one of: Car | Pedestrian | Scooter |
//!   Truck) — Specifies the mode of transport when calculating a route. Used in estimating
//!   the speed of travel and road compatibility. Default value: Car
//! * `TravelModeOptions` (WaypointOptimizationTravelModeOptions) — Travel mode related
//!   options for the provided travel mode.
//! * `Waypoints` (WaypointOptimizationWaypointList) — List of waypoints between the Origin
//!   and Destination, in World Geodetic System (WGS 84) format: [longitude, latitude]. The
//!   maximum number of waypoints allowed per request: Maximum 50 waypoints per request
//!   Maximum 20 waypoints when using constraints (AccessHours, AppointmentTime,
//!   ServiceDuration, Heading, SideOfStreet, Before)
//!
//! ## Output
//!
//! * `Connections` (WaypointOptimizationConnectionList) **required** — Details about the
//!   connection from one waypoint to the next, within the optimized sequence.
//! * `Distance` (DistanceMeters, range 0..4294967295, default 0) **required** — Overall
//!   distance to travel the whole sequence.
//! * `Duration` (DurationSeconds, range 0..4294967295, default 0) **required** — Overall
//!   duration to travel the whole sequence. Unit: seconds
//! * `ImpedingWaypoints` (WaypointOptimizationImpedingWaypointList) **required** — Returns
//!   waypoints that caused the optimization problem to fail, and the constraints that were
//!   unsatisfied leading to the failure.
//! * `OptimizedWaypoints` (WaypointOptimizationOptimizedWaypointList) **required** —
//!   Waypoints in the order of the optimized sequence.
//! * `PricingBucket` (String) **required** — The pricing bucket for which the query is
//!   charged at.
//! * `TimeBreakdown` (WaypointOptimizationTimeBreakdown) **required** — Time breakdown for
//!   the sequence.
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

use crate::state::{self, Instant, Position, RoutingObjective, TravelMode};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `OptimizeWaypointsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-georoutes-routesservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `OptimizeWaypointsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.georoutes#OptimizeWaypoints")]
#[derive(Debug)]
pub enum OptimizeWaypointsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::optimize_waypoints::OptimizeWaypointsError`). The wrapper is kept
/// because `Clustering` and `Driver.RestProfile` are answered with its 501.
pub type OptimizeWaypointsError = crate::scaffolded::Scaffolded<OptimizeWaypointsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<OptimizeWaypointsErrorModeled> for OptimizeWaypointsError {
    fn from(modeled: OptimizeWaypointsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `OptimizeWaypoints` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct OptimizeWaypoints<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> OptimizeWaypoints<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for OptimizeWaypoints<Eng>
{
    type Input = OptimizeWaypointsInput;
    type Output = OptimizeWaypointsOutput;
    type Error = OptimizeWaypointsError;

    const NAME: &'static str = "OptimizeWaypoints";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::OPTIMIZE_WAYPOINTS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::OPTIMIZE_WAYPOINTS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;

        if input.clustering.is_some() {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "OptimizeWaypoints with Clustering",
            ));
        }
        if input.driver.as_ref().is_some_and(|driver| driver.rest_profile.is_some()) {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "OptimizeWaypoints with Driver.RestProfile",
            ));
        }
        // "This setting mandates that the router will avoid any routes that include the
        // specified options": no road of this model carries the country it is in, so a
        // country exclusion can be neither honoured nor reported.
        if input.exclude.is_some() {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "OptimizeWaypoints with Exclude",
            ));
        }
        // An avoidance is best-effort and a violated one is reported as a notice — but this
        // answer has nowhere to carry one, so the two avoidances this road model could
        // violate are declined instead of silently violated. Everything else `Avoid` names
        // is satisfied trivially: this network carries no toll road, ferry, tunnel, dirt
        // road or car shuttle train, and the computed path never turns back on itself.
        if let Some(avoid) = input.avoid.as_ref() {
            if avoid.areas.as_deref().is_some_and(|areas| !areas.is_empty()) {
                return Err(crate::scaffolded::Scaffolded::not_implemented(
                    "OptimizeWaypoints with Avoid.Areas",
                ));
            }
            if avoid.controlled_access_highways == Some(true) {
                return Err(crate::scaffolded::Scaffolded::not_implemented(
                    "OptimizeWaypoints with Avoid.ControlledAccessHighways",
                ));
            }
        }
        // An access window would move the arrival, the wait and the impeding waypoints,
        // and this slice computes none of that from it, so it is declined rather than
        // accepted and ignored.
        let access_hours_set = input
            .waypoints
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|waypoint| waypoint.access_hours.is_some())
            || input
                .destination_options
                .as_ref()
                .is_some_and(|options| options.access_hours.is_some());
        if access_hours_set {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "OptimizeWaypoints with AccessHours",
            ));
        }

        let travel_mode = match input.travel_mode.as_deref() {
            // `Default value: Car`.
            None => TravelMode::Car,
            Some(value) => TravelMode::from_model_values(value, WAYPOINT_OPTIMIZATION_TRAVEL_MODE_VALUES)
                .ok_or_else(|| {
                    invalid(
                        "TravelMode",
                        format!("TravelMode {value} is not a WaypointOptimizationTravelMode"),
                    )
                })?,
        };
        let objective = match input.optimize_sequencing_for.as_deref() {
            None => RoutingObjective::FastestRoute,
            Some(value) => RoutingObjective::from_model(value).ok_or_else(|| {
                invalid(
                    "OptimizeSequencingFor",
                    format!("OptimizeSequencingFor {value} is not a WaypointOptimizationSequencingObjective"),
                )
            })?,
        };

        let departure = match &input.departure_time {
            Some(value) => {
                state::parse_timestamp("DepartureTime", value).map_err(|message| invalid("DepartureTime", message))?
            }
            None => state::now(),
        };

        let submitted = input.waypoints.as_deref().unwrap_or_default();

        // The documented server-side maximum, which the member carries no modeled length
        // constraint for: "Maximum 50 waypoints per request / Maximum 20 waypoints when
        // using constraints (AccessHours, AppointmentTime, ServiceDuration, Heading,
        // SideOfStreet, Before)". It is enforced here because the sequencer below costs
        // every remaining candidate at every step, so an unbounded list would be
        // unbounded work for one request.
        let uses_constraints = submitted.iter().any(|waypoint| {
            waypoint.access_hours.is_some()
                || waypoint.appointment_time.is_some()
                || waypoint.side_of_street.is_some()
                || waypoint.service_duration != 0
                || waypoint.heading != 0.0
                || waypoint.before.as_deref().is_some_and(|before| !before.is_empty())
        });
        let maximum = if uses_constraints {
            MAX_CONSTRAINED_WAYPOINTS
        } else {
            MAX_WAYPOINTS
        };
        if submitted.len() > maximum {
            return Err(invalid(
                "Waypoints",
                format!(
                    "at most {maximum} waypoints are allowed{}; got {}",
                    if uses_constraints { " when using constraints" } else { "" },
                    submitted.len()
                ),
            ));
        }

        // `ServiceDuration` and `Heading` carry modeled ranges the boundary validator does
        // not enforce on a defaulted member, and the service duration is added to the
        // sequence's clock, so the handler checks both.
        for (index, waypoint) in submitted.iter().enumerate() {
            check_service_duration(&format!("Waypoints[{index}].ServiceDuration"), waypoint.service_duration)?;
            check_heading(&format!("Waypoints[{index}].Heading"), waypoint.heading)?;
        }
        if let Some(options) = input.destination_options.as_ref() {
            check_service_duration("DestinationOptions.ServiceDuration", options.service_duration)?;
            check_heading("DestinationOptions.Heading", options.heading)?;
        }

        // A `Before` entry names another waypoint by its index, so an index outside the
        // submitted list, or a waypoint's own, names no constraint this service can honour
        // or report, and the same index twice names one constraint twice.
        for (index, waypoint) in submitted.iter().enumerate() {
            let before = waypoint.before.as_deref().unwrap_or_default();
            for (position, entry) in before.iter().enumerate() {
                let member = format!("Waypoints[{index}].Before");
                let named = usize::try_from(*entry).ok().filter(|named| *named != index);
                if named.is_none_or(|named| named >= submitted.len()) {
                    return Err(invalid(
                        &member,
                        format!(
                            "Before must name another waypoint by its index in Waypoints (0..{}); got {entry}",
                            submitted.len()
                        ),
                    ));
                }
                if before[position + 1..].contains(entry) {
                    return Err(invalid(
                        &member,
                        format!("Before must name each waypoint at most once; {entry} is named more than once"),
                    ));
                }
            }
        }

        // Every connection names its endpoints by id, so two stops sharing one id would
        // make the answer ambiguous: a duplicate among the ids the CALLER supplied is
        // refused, and an id this service has to make up for a stop the caller left
        // unnamed is chosen so that it cannot collide with one of them.
        let supplied_origin_id = input
            .origin_options
            .as_ref()
            .and_then(|options| options.id.clone());
        let supplied_destination_id = input
            .destination_options
            .as_ref()
            .and_then(|options| options.id.clone());
        let supplied: Vec<String> = [supplied_origin_id.clone()]
            .into_iter()
            .chain(submitted.iter().map(|waypoint| waypoint.id.clone()))
            .chain([supplied_destination_id.clone()])
            .flatten()
            .collect();
        for (index, id) in supplied.iter().enumerate() {
            if supplied[index + 1..].contains(id) {
                return Err(invalid(
                    "Waypoints",
                    format!("waypoint ids must be unique within a request; {id} is used more than once"),
                ));
            }
        }
        let mut taken = supplied;
        let origin_id = assign_id(supplied_origin_id, "Origin", &mut taken);
        let waypoint_ids: Vec<String> = submitted
            .iter()
            .enumerate()
            .map(|(index, waypoint)| assign_id(waypoint.id.clone(), &index.to_string(), &mut taken))
            .collect();
        let destination_id = assign_id(supplied_destination_id, "Destination", &mut taken);

        // The origin, every waypoint and the destination, as the sequencer sees them.
        let origin = Stop {
            id: origin_id,
            position: state::validate_position("Origin", &input.origin)
                .map_err(|message| invalid("Origin", message))?,
            service_duration_s: 0,
            appointment: None,
            before: Vec::new(),
            waypoint_index: None,
        };
        let mut waypoints = Vec::new();
        for (index, waypoint) in submitted.iter().enumerate() {
            let member = format!("Waypoints[{index}].Position");
            waypoints.push(Stop {
                id: waypoint_ids[index].clone(),
                position: state::validate_position(&member, &waypoint.position)
                    .map_err(|message| invalid(&member, message))?,
                service_duration_s: waypoint.service_duration,
                appointment: match &waypoint.appointment_time {
                    None => None,
                    Some(value) => {
                        let member = format!("Waypoints[{index}].AppointmentTime");
                        Some(
                            state::parse_timestamp(&member, value)
                                .map_err(|message| invalid(&member, message))?,
                        )
                    }
                },
                before: waypoint
                    .before
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|index| usize::try_from(*index).ok())
                    .collect(),
                waypoint_index: Some(index),
            });
        }
        let destination = match &input.destination {
            None => None,
            Some(position) => {
                let options = input.destination_options.as_ref();
                Some(Stop {
                    id: destination_id,
                    position: state::validate_position("Destination", position)
                        .map_err(|message| invalid("Destination", message))?,
                    service_duration_s: options.map_or(0, |options| options.service_duration),
                    appointment: match options.and_then(|options| options.appointment_time.clone()) {
                        None => None,
                        Some(value) => Some(
                            state::parse_timestamp("DestinationOptions.AppointmentTime", &value)
                                .map_err(|message| invalid("DestinationOptions.AppointmentTime", message))?,
                        ),
                    },
                    before: Vec::new(),
                    waypoint_index: None,
                })
            }
        };

        let state_handle =
            state::service_state(&*self.engine).ok_or_else(|| internal("the routes service state is unavailable"))?;

        // Sequence: from the stop reached last, take the cheapest unvisited waypoint the
        // `Before` constraints allow — a waypoint another unvisited one requires to be
        // visited after it is not eligible yet. When no candidate is eligible the
        // constraints cannot all hold (they name a cycle), so the step falls back to cost
        // alone and the broken ones are reported under `ImpedingWaypoints` below.
        //
        // The lock on the shared state is taken once per step — at most `Waypoints.len()`
        // route computations, itself capped by the documented maximum above — and released
        // between steps, where the handler yields, so a long sequence neither holds the
        // lock for the whole request nor leaves a client's cancellation nowhere to land.
        let mut sequence = vec![origin];
        let mut remaining: Vec<Stop> = waypoints;
        while !remaining.is_empty() {
            let from = sequence
                .last()
                .ok_or_else(|| internal("the optimized sequence lost its origin"))?
                .position;
            let eligible: Vec<usize> = (0..remaining.len())
                .filter(|&index| {
                    let waypoint_index = remaining[index].waypoint_index;
                    !remaining.iter().enumerate().any(|(other, stop)| {
                        other != index && waypoint_index.is_some_and(|named| stop.before.contains(&named))
                    })
                })
                .collect();
            let pool: Vec<usize> = if eligible.is_empty() {
                (0..remaining.len()).collect()
            } else {
                eligible
            };
            let chosen = {
                let mut state = state_handle.lock();
                let mut best: Option<(usize, i64)> = None;
                for index in pool {
                    let candidate = &remaining[index];
                    let route = state.compute_route(&[from, candidate.position], travel_mode, objective, 0);
                    let cost = match objective {
                        RoutingObjective::FastestRoute => route.duration_s,
                        RoutingObjective::ShortestRoute => route.distance_m,
                    };
                    if best.is_none_or(|(_, best_cost)| cost < best_cost) {
                        best = Some((index, cost));
                    }
                }
                best.map(|(index, _)| index)
            };
            let index = chosen.ok_or_else(|| internal("no next waypoint could be chosen"))?;
            sequence.push(remaining.remove(index));
            tokio::task::yield_now().await;
        }
        sequence.extend(destination);

        // The cost of each hop of the computed sequence, from the same cost function.
        let (routed, pricing_bucket) = {
            let mut state = state_handle.lock();
            let routed: Vec<(i64, i64)> = sequence
                .windows(2)
                .map(|pair| {
                    let route =
                        state.compute_route(&[pair[0].position, pair[1].position], travel_mode, objective, 0);
                    (route.distance_m, route.duration_s)
                })
                .collect();
            (routed, state.pricing_bucket.clone())
        };

        let treat_service_time_as_rest = input
            .driver
            .as_ref()
            .and_then(|driver| driver.treat_service_time_as.as_deref())
            == Some("Rest");
        let rest_cycles = input.driver.as_ref().and_then(|driver| driver.rest_cycles.clone());

        let mut connections = Vec::new();
        let mut optimized = Vec::new();
        let mut impeding = Vec::new();
        let mut breakdown = WaypointOptimizationTimeBreakdown::default();
        let mut schedule = RestSchedule::default();
        let mut clock = departure;

        optimized.push(WaypointOptimizationOptimizedWaypoint {
            arrival_time: None,
            cluster_index: None,
            departure_time: state::format_timestamp(clock)
                .ok_or_else(|| internal("the departure time is not representable"))?,
            id: sequence[0].id.clone(),
            position: state::position_to_vec(sequence[0].position),
        });

        for (index, pair) in sequence.windows(2).enumerate() {
            let (from, to) = (&pair[0], &pair[1]);
            let (distance_m, travel_duration) = routed[index];
            let rest_duration = schedule.rest_after(travel_duration, rest_cycles.as_ref());
            let arrival = state::shift_seconds(clock, travel_duration.saturating_add(rest_duration));
            // An appointment the traveller reaches early is waited out; one already past
            // is a failed `AppointmentTime` constraint, not a negative wait.
            let wait_duration: i64 = to
                .appointment
                .map_or(0, |appointment| state::seconds_between(arrival, appointment).max(0));
            let service_duration = to.service_duration_s;
            if treat_service_time_as_rest {
                schedule.rest(service_duration, rest_cycles.as_ref());
            } else {
                schedule.work(service_duration);
            }
            let departure_from_to =
                state::shift_seconds(arrival, wait_duration.saturating_add(service_duration));

            connections.push(WaypointOptimizationConnection {
                distance: distance_m,
                from: from.id.clone(),
                rest_duration,
                to: to.id.clone(),
                travel_duration,
                wait_duration,
            });
            optimized.push(WaypointOptimizationOptimizedWaypoint {
                arrival_time: state::format_timestamp(arrival),
                // No clustering was performed, so no cluster index is reported.
                cluster_index: None,
                departure_time: state::format_timestamp(departure_from_to)
                    .ok_or_else(|| internal("a departure time is not representable"))?,
                id: to.id.clone(),
                position: state::position_to_vec(to.position),
            });

            breakdown.travel_duration =
                state::clamp_cost(breakdown.travel_duration.saturating_add(travel_duration) as f64);
            breakdown.rest_duration = state::clamp_cost(breakdown.rest_duration.saturating_add(rest_duration) as f64);
            breakdown.wait_duration = state::clamp_cost(breakdown.wait_duration.saturating_add(wait_duration) as f64);
            breakdown.service_duration =
                state::clamp_cost(breakdown.service_duration.saturating_add(service_duration) as f64);

            if let Some(appointment) = to.appointment {
                if arrival > appointment {
                    impeding.push(impeding_waypoint(
                        to,
                        "AppointmentTime",
                        format!(
                            "the computed arrival is {} seconds after the appointment",
                            state::seconds_between(appointment, arrival)
                        ),
                    ));
                }
            }
            clock = departure_from_to;
        }

        // `Before` says which waypoints must be visited after this one. The sequencer
        // honours it: a waypoint another unvisited one requires to come later is not
        // eligible yet. A cyclic constraint set cannot be satisfied by any order, so the
        // sequencer falls back to cost and the constraints left unmet are reported here as
        // `ImpedingWaypoints` — which is also the check that the honoured case passes.
        let order: Vec<Option<usize>> = sequence.iter().map(|stop| stop.waypoint_index).collect();
        for (place, stop) in sequence.iter().enumerate() {
            for required_after in &stop.before {
                let later = order
                    .iter()
                    .position(|index| *index == Some(*required_after))
                    .is_some_and(|position| position > place);
                if !later {
                    impeding.push(impeding_waypoint(
                        stop,
                        "Before",
                        format!("Waypoints[{required_after}] is not sequenced after this waypoint"),
                    ));
                }
            }
        }

        Ok(OptimizeWaypointsOutput {
            distance: state::clamp_cost(
                connections
                    .iter()
                    .map(|c| c.distance)
                    .fold(0_i64, i64::saturating_add) as f64,
            ),
            duration: state::clamp_cost(
                connections
                    .iter()
                    .map(|c| {
                        c.travel_duration
                            .saturating_add(c.rest_duration)
                            .saturating_add(c.wait_duration)
                    })
                    .fold(0_i64, i64::saturating_add) as f64,
            ),
            connections,
            impeding_waypoints: impeding,
            optimized_waypoints: optimized,
            pricing_bucket,
            time_breakdown: breakdown,
        })
    }
}

/// The documented maximum number of waypoints per request, and the lower maximum that
/// applies when any waypoint uses a constraint.
const MAX_WAYPOINTS: usize = 50;
const MAX_CONSTRAINED_WAYPOINTS: usize = 20;

/// The upper bound of the model's `DurationSeconds` range, which `ServiceDuration`
/// targets (`0..4294967295`).
const MAX_SERVICE_DURATION_S: i64 = 4_294_967_295;

/// `Heading`'s modeled range, in degrees.
const HEADING_BOUNDS: (f64, f64) = (0.0, 360.0);

/// Refuse a service duration outside the range the model declares for it.
fn check_service_duration(member: &str, seconds: i64) -> Result<(), OptimizeWaypointsError> {
    if (0..=MAX_SERVICE_DURATION_S).contains(&seconds) {
        return Ok(());
    }
    Err(invalid(
        member,
        format!("ServiceDuration must be in [0, {MAX_SERVICE_DURATION_S}] seconds; got {seconds}"),
    ))
}

/// Refuse a heading outside the range the model declares for it.
fn check_heading(member: &str, heading: f64) -> Result<(), OptimizeWaypointsError> {
    if heading.is_finite() && (HEADING_BOUNDS.0..=HEADING_BOUNDS.1).contains(&heading) {
        return Ok(());
    }
    Err(invalid(
        member,
        format!(
            "Heading must be in [{}, {}] degrees; got {heading}",
            HEADING_BOUNDS.0, HEADING_BOUNDS.1
        ),
    ))
}

/// The id of a stop: the one the caller supplied, or `preferred` made unique against the
/// ids already in use. Recorded in `taken`, so two made-up ids cannot collide either.
fn assign_id(supplied: Option<String>, preferred: &str, taken: &mut Vec<String>) -> String {
    if let Some(id) = supplied {
        return id;
    }
    let mut candidate = preferred.to_string();
    let mut suffix = 2;
    while taken.contains(&candidate) {
        candidate = format!("{preferred}-{suffix}");
        suffix += 1;
    }
    taken.push(candidate.clone());
    candidate
}

/// One stop of the sequence: the origin, a waypoint or the destination.
#[derive(Debug, Clone)]
struct Stop {
    id: String,
    position: Position,
    service_duration_s: i64,
    appointment: Option<Instant>,
    /// `Before`: indices in `Waypoints` that must be visited after this stop.
    before: Vec<usize>,
    /// Index in the request's `Waypoints` list, for the stops that have one.
    waypoint_index: Option<usize>,
}

/// The driver's work-rest state while the sequence is walked.
///
/// "A rest needs to be taken after the short work duration. The short cycle can be
/// repeated until you hit the long work duration, at which point the long rest duration is
/// taken."
#[derive(Debug, Default)]
struct RestSchedule {
    work_since_short_rest: i64,
    work_since_long_rest: i64,
}

impl RestSchedule {
    /// Count `seconds` of work, without resting.
    fn work(&mut self, seconds: i64) {
        self.work_since_short_rest = self.work_since_short_rest.saturating_add(seconds);
        self.work_since_long_rest = self.work_since_long_rest.saturating_add(seconds);
    }

    /// Count `seconds` of rest: any rest starts the short cycle again, and a rest as long
    /// as the long cycle's own rest starts that cycle again too.
    fn rest(&mut self, seconds: i64, cycles: Option<&WaypointOptimizationRestCycles>) {
        if seconds <= 0 {
            return;
        }
        self.work_since_short_rest = 0;
        if cycles.is_some_and(|cycles| {
            cycles.long_cycle.rest_duration > 0 && seconds >= cycles.long_cycle.rest_duration
        }) {
            self.work_since_long_rest = 0;
        }
    }

    /// Work `travel` seconds, then the rest the schedule now owes, in seconds.
    fn rest_after(&mut self, travel: i64, cycles: Option<&WaypointOptimizationRestCycles>) -> i64 {
        self.work(travel);
        let Some(cycles) = cycles else {
            return 0;
        };
        if cycles.long_cycle.work_duration > 0 && self.work_since_long_rest >= cycles.long_cycle.work_duration {
            self.work_since_long_rest = 0;
            self.work_since_short_rest = 0;
            return cycles.long_cycle.rest_duration.max(0);
        }
        if cycles.short_cycle.work_duration > 0 && self.work_since_short_rest >= cycles.short_cycle.work_duration {
            self.work_since_short_rest = 0;
            return cycles.short_cycle.rest_duration.max(0);
        }
        0
    }
}

/// A waypoint the sequencer could not satisfy a constraint for.
fn impeding_waypoint(stop: &Stop, constraint: &str, reason: String) -> WaypointOptimizationImpedingWaypoint {
    WaypointOptimizationImpedingWaypoint {
        failed_constraints: vec![WaypointOptimizationFailedConstraint {
            constraint: Some(constraint.to_string()),
            reason: Some(reason),
        }],
        id: stop.id.clone(),
        position: state::position_to_vec(stop.position),
    }
}

/// The model's own `ValidationException` (HTTP 400) for a request member this service
/// refuses, naming the member and the rule it broke.
fn invalid(member: &str, message: String) -> OptimizeWaypointsError {
    OptimizeWaypointsErrorModeled::ValidationException(ValidationException {
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
fn internal(message: &str) -> OptimizeWaypointsError {
    OptimizeWaypointsErrorModeled::InternalServerException(InternalServerException {
        message: message.to_string(),
    })
    .into()
}
