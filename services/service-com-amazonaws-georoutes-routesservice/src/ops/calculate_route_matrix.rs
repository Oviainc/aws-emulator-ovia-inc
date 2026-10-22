// SPDX-License-Identifier: CC0-1.0

//! @op CalculateRouteMatrix
//!
//! One row per origin, one entry per destination in each row. Every entry is costed by
//! the same [`ComAmazonawsGeoroutesRoutesserviceState::compute_route`] call CalculateRoutes
//! makes for that pair, so `RouteMatrix[i][j]` and a direct route for
//! `(Origins[i], Destinations[j])` report the same distance and duration.
//!
//! A pair whose origin or destination falls outside the routing boundary is not routed:
//! its entry carries the `OutOfBoundsOrigin` / `OutOfBoundsDestination` error code and
//! `ErrorCount` counts those entries. Containment is decided once per submitted position,
//! not once per pair.
//!
//! `Exclude` answers 501: a country exclusion is "strictly excluded" and no road of the
//! model carries the country it is in. So do `Avoid.Areas` and
//! `Avoid.ControlledAccessHighways`: an avoidance is best-effort and a violated one
//! "produces a notice for the violation", and a matrix entry has nowhere to carry a
//! notice. The rest of `Avoid` is satisfied trivially (this network has no toll road,
//! ferry, tunnel, dirt road, car shuttle train or zone), and `Allow`, `Traffic` and
//! `TravelModeOptions` have nothing to act on for the same reason: the model carries no
//! HOV/HOT lane, traffic variation or vehicle restriction. `DepartureTime` and `DepartNow`
//! are validated but change nothing, because no cost here varies with the time of day.
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
//! # Modeled contract for `CalculateRouteMatrix`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-georoutes-routesservice/smithy-model/com-amazonaws-georoutes-routesservice.json")
//! ```
//!
//! Use CalculateRouteMatrix to compute results for all pairs of Origins to Destinations.
//! Each row corresponds to one entry in Origins. Each entry in the row corresponds to the
//! route from that entry in Origins to an entry in Destinations positions. For more
//! information, see Calculate route matrix in the Amazon Location Service Developer Guide.
//!
//! ## Input
//!
//! * `Allow` (RouteMatrixAllowOptions) — Features that are allowed while calculating a route.
//! * `Avoid` (RouteMatrixAvoidanceOptions) — Features that are avoided while calculating a
//!   route. Avoidance is on a best-case basis. If an avoidance can't be satisfied for a
//!   particular case, it violates the avoidance and the returned response produces a notice
//!   for the violation. For GrabMaps customers, ap-southeast-1 and ap-southeast-5 regions
//!   support only TollRoads, Ferries, and ControlledAccessHighways.
//! * `DepartNow` (SensitiveBoolean) — Uses the current time as the time of departure.
//! * `DepartureTime` (TimestampWithTimezoneOffset, pattern
//!   ^([1-2][0-9]{3})-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):([0-5][0-9]):([0-5][0-9]|60)(\.[0-9]{0,9})?
//!   …) — Time of departure from the origin. Time format:YYYY-MM-DDThh:mm:ss.sssZ |
//!   YYYY-MM-DDThh:mm:ss.sss+hh:mm Examples: 2020-04-22T17:57:24Z 2020-04-22T17:57:24+02:00
//! * `Destinations` (RouteMatrixDestinationList) **required** — List of destinations for the
//!   route in World Geodetic System (WGS 84) format: [longitude, latitude]. Route
//!   calculations are billed for each origin and destination pair. If you use a large matrix
//!   of origins and destinations, your costs will increase accordingly. For more information,
//!   see Routes pricing in the Amazon Location Service Developer Guide. The maximum number of
//!   destinations depends on the routing boundary configuration: With
//!   RoutingBoundary.Geometry set: maximum 500 destinations With RoutingBoundary.Unbounded
//!   set to true: maximum 100 destinations For GrabMaps customers in ap-southeast-1 and
//!   ap-southeast-5: maximum 350 destinations The total matrix size (origins × destinations)
//!   must …
//! * `Exclude` (RouteMatrixExclusionOptions) — Features to be strictly excluded while
//!   calculating the route. Not supported in ap-southeast-1 and ap-southeast-5 regions for
//!   GrabMaps customers.
//! * `Key` (ApiKey, len ..1000) — Optional: The API key to be used for authorization. Either
//!   an API key or valid SigV4 signature must be provided when making a request.
//! * `OptimizeRoutingFor` (RoutingObjective, one of: FastestRoute | ShortestRoute) — Controls
//!   the trade-off between finding the shortest travel time (FastestRoute) and the shortest
//!   distance (ShortestRoute) when calculating reachable areas. Default value: FastestRoute
//! * `Origins` (RouteMatrixOriginList) **required** — List of origins for the route in World
//!   Geodetic System (WGS 84) format: [longitude, latitude]. Route calculations are billed
//!   for each origin and destination pair. Using a large amount of Origins in a request can
//!   lead you to incur unexpected charges. For more information, see Routes pricing in the
//!   Amazon Location Service Developer Guide. The maximum number of origins depends on the
//!   routing boundary configuration: With RoutingBoundary.Geometry set: maximum 500 origins
//!   With RoutingBoundary.Unbounded set to true: maximum 15 origins For GrabMaps customers in
//!   ap-southeast-1 and ap-southeast-5: maximum 350 origins The total matrix size (origins ×
//!   destinations) must not exceed: With …
//! * `RoutingBoundary` (RouteMatrixBoundary) — Boundary within which the matrix is to be
//!   calculated. All data, origins and destinations outside the boundary are considered
//!   invalid. For GrabMaps customers, ap-southeast-1 and ap-southeast-5 regions support only
//!   Unbounded set to true. Default value: Unbounded set to true When AutoCircle is set in
//!   the request, the response routing boundary will return Circle derived from the
//!   AutoCircle settings.
//! * `Traffic` (RouteMatrixTrafficOptions) — Traffic related options. Not supported in
//!   ap-southeast-1 and ap-southeast-5 regions for GrabMaps customers.
//! * `TravelMode` (RouteMatrixTravelMode, one of: Car | Pedestrian | Scooter | Truck) —
//!   Specifies the mode of transport when calculating a route. Used in estimating the speed
//!   of travel and road compatibility. For GrabMaps customers, ap-southeast-1 and
//!   ap-southeast-5 regions support only Car, Pedestrian, and Scooter. Default value: Car
//! * `TravelModeOptions` (RouteMatrixTravelModeOptions) — Travel mode related options for the
//!   provided travel mode. Not supported in ap-southeast-1 and ap-southeast-5 regions for
//!   GrabMaps customers.
//!
//! ## Output
//!
//! * `ErrorCount` (Integer) **required** — The count of error results in the route matrix. If
//!   this number is 0, all routes were calculated successfully.
//! * `PricingBucket` (String) **required** — The pricing bucket for which the query is
//!   charged at.
//! * `RouteMatrix` (RouteMatrix) **required** — The calculated route matrix containing the
//!   results for all pairs of Origins to Destination positions. Each row corresponds to one
//!   entry in Origins. Each entry in the row corresponds to the route from that entry in
//!   Origins to an entry in Destination positions.
//! * `RoutingBoundary` (RouteMatrixBoundary) **required** — Boundary within which the matrix
//!   is to be calculated. All data, origins and destinations outside the boundary are
//!   considered invalid. When AutoCircle is set in the request, the response routing boundary
//!   will return Circle derived from the AutoCircle settings.
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

use crate::state::{self, Position, RoutingObjective, TravelMode};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CalculateRouteMatrixError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-georoutes-routesservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CalculateRouteMatrixError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.georoutes#CalculateRouteMatrix")]
#[derive(Debug)]
pub enum CalculateRouteMatrixErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::calculate_route_matrix::CalculateRouteMatrixError`). The wrapper is kept
/// because it is also the only place a request this operation models no error for could be
/// refused; every refusal below uses the model's own `ValidationException`.
pub type CalculateRouteMatrixError = crate::scaffolded::Scaffolded<CalculateRouteMatrixErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CalculateRouteMatrixErrorModeled> for CalculateRouteMatrixError {
    fn from(modeled: CalculateRouteMatrixErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CalculateRouteMatrix` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CalculateRouteMatrix<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CalculateRouteMatrix<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CalculateRouteMatrix<Eng>
{
    type Input = CalculateRouteMatrixInput;
    type Output = CalculateRouteMatrixOutput;
    type Error = CalculateRouteMatrixError;

    const NAME: &'static str = "CalculateRouteMatrix";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CALCULATE_ROUTE_MATRIX_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CALCULATE_ROUTE_MATRIX_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;

        // "This setting mandates that the router will avoid any routes that include the
        // specified options": no road of this model carries the country it is in, so a
        // country exclusion can be neither honoured nor reported.
        if input.exclude.is_some() {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "CalculateRouteMatrix with Exclude",
            ));
        }
        // An avoidance is best-effort and a violated one "produces a notice for the
        // violation" — but a matrix entry has nowhere to carry a notice, so the two
        // avoidances this road model could violate (an area it would have to route around,
        // and the controlled-access highways it does carry) are declined instead of
        // silently violated. Everything else `Avoid` names is satisfied trivially: this
        // network carries no toll road, ferry, tunnel, dirt road, car shuttle train or
        // zone, and the computed path never turns back on itself.
        if let Some(avoid) = input.avoid.as_ref() {
            if avoid.areas.as_deref().is_some_and(|areas| !areas.is_empty()) {
                return Err(crate::scaffolded::Scaffolded::not_implemented(
                    "CalculateRouteMatrix with Avoid.Areas",
                ));
            }
            if avoid.controlled_access_highways == Some(true) {
                return Err(crate::scaffolded::Scaffolded::not_implemented(
                    "CalculateRouteMatrix with Avoid.ControlledAccessHighways",
                ));
            }
        }

        let travel_mode = match input.travel_mode.as_deref() {
            // `Default value: Car`.
            None => TravelMode::Car,
            Some(value) => TravelMode::from_model_values(value, ROUTE_MATRIX_TRAVEL_MODE_VALUES).ok_or_else(|| {
                invalid(
                    "TravelMode",
                    format!("TravelMode {value} is not a RouteMatrixTravelMode"),
                )
            })?,
        };
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
        if input.depart_now == Some(true) && input.departure_time.is_some() {
            return Err(invalid(
                "DepartNow",
                "DepartNow and DepartureTime each set the time of departure, so at most one may be supplied"
                    .to_string(),
            ));
        }
        if let Some(departure) = &input.departure_time {
            state::parse_timestamp("DepartureTime", departure).map_err(|message| invalid("DepartureTime", message))?;
        }

        // The model pins `Origins`/`Destinations` non-empty; the positions in them are
        // this handler's to check.
        let mut origins = Vec::with_capacity(input.origins.len());
        for (index, origin) in input.origins.iter().enumerate() {
            let member = format!("Origins[{index}].Position");
            origins.push(
                state::validate_position(&member, &origin.position).map_err(|message| invalid(&member, message))?,
            );
        }
        let mut destinations = Vec::with_capacity(input.destinations.len());
        for (index, destination) in input.destinations.iter().enumerate() {
            let member = format!("Destinations[{index}].Position");
            destinations.push(
                state::validate_position(&member, &destination.position)
                    .map_err(|message| invalid(&member, message))?,
            );
        }

        let (boundary, reported_boundary) = resolve_boundary(&input, &origins, &destinations)?;
        check_matrix_size(&boundary, origins.len(), destinations.len())?;

        let state_handle =
            state::service_state(&*self.engine).ok_or_else(|| internal("the routes service state is unavailable"))?;

        // Boundary containment is decided ONCE per submitted position — not once per pair —
        // and before any lock is taken, so the per-pair work is one route computation and
        // nothing else.
        let origin_inside: Vec<bool> = origins.iter().map(|origin| boundary.contains(*origin)).collect();
        let destination_inside: Vec<bool> = destinations
            .iter()
            .map(|destination| boundary.contains(*destination))
            .collect();

        // The matrix is costed ONE ROW at a time: the lock on the shared state is taken
        // per row (at most `Destinations.len()` route computations, itself capped by the
        // documented limits above) and released between rows, and the handler yields there
        // — so a large matrix neither holds the lock for the whole request nor leaves a
        // client's cancellation nowhere to land.
        let mut route_matrix = Vec::with_capacity(origins.len());
        let mut error_count: i32 = 0;
        for (i, origin) in origins.iter().enumerate() {
            let mut row = Vec::with_capacity(destinations.len());
            {
                let mut state = state_handle.lock();
                for (j, destination) in destinations.iter().enumerate() {
                    // Out of the boundary is not routed: "All data, origins and
                    // destinations outside the boundary are considered invalid."
                    let error = if origin_inside[i] {
                        if destination_inside[j] {
                            None
                        } else {
                            Some("OutOfBoundsDestination")
                        }
                    } else {
                        Some("OutOfBoundsOrigin")
                    };
                    if let Some(code) = error {
                        error_count = error_count.saturating_add(1);
                        row.push(RouteMatrixEntry {
                            distance: 0,
                            duration: 0,
                            error: Some(code.to_string()),
                        });
                        continue;
                    }
                    let route = state.compute_route(&[*origin, *destination], travel_mode, objective, 0);
                    row.push(RouteMatrixEntry {
                        distance: route.distance_m,
                        duration: route.duration_s,
                        error: None,
                    });
                }
            }
            route_matrix.push(row);
            tokio::task::yield_now().await;
        }
        let pricing_bucket = state_handle.lock().pricing_bucket.clone();

        Ok(CalculateRouteMatrixOutput {
            error_count,
            pricing_bucket,
            route_matrix,
            routing_boundary: reported_boundary,
        })
    }
}

/// The model's own `ValidationException` (HTTP 400) for a request member this service
/// refuses, naming the member and the rule it broke.
fn invalid(member: &str, message: String) -> CalculateRouteMatrixError {
    CalculateRouteMatrixErrorModeled::ValidationException(ValidationException {
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
fn internal(message: &str) -> CalculateRouteMatrixError {
    CalculateRouteMatrixErrorModeled::InternalServerException(InternalServerException {
        message: message.to_string(),
    })
    .into()
}

/// Documented upper bound on the radius of a routing boundary circle and on the
/// `AutoCircle` margin plus maximum radius, in metres.
const MAX_BOUNDARY_RADIUS_M: f64 = 200_000.0;

/// Documented matrix limits: `RoutingBoundary.Geometry` allows 500 origins, 500
/// destinations and 160,000 pairs; `RoutingBoundary.Unbounded` allows 15 origins, 100
/// destinations and 100 pairs.
const GEOMETRY_LIMITS: (usize, usize, usize) = (500, 500, 160_000);
const UNBOUNDED_LIMITS: (usize, usize, usize) = (15, 100, 100);

/// Most positions a routing-boundary linear ring may carry. The model pins only its lower
/// bound (four positions, closed) and AWS documents no upper one, so this is this
/// emulator's own bound on the work one request's boundary can cost.
const MAX_RING_POSITIONS: usize = 1_000;

/// The area a matrix is computed within.
#[derive(Debug, Clone)]
enum Boundary {
    /// "No restrictions in terms of a routing boundary" — the documented default.
    Unbounded,
    Circle { center: Position, radius_m: f64 },
    BoundingBox { west: f64, south: f64, east: f64, north: f64 },
    /// One linear ring, as `Polygon` carries it.
    Polygon(Vec<Position>),
}

impl Boundary {
    /// Whether `position` is inside the boundary.
    fn contains(&self, position: Position) -> bool {
        match self {
            Self::Unbounded => true,
            Self::Circle { center, radius_m } => state::haversine_m(*center, position) <= *radius_m,
            Self::BoundingBox {
                west,
                south,
                east,
                north,
            } => {
                // "The first pair represents the X and Y coordinates (longitude and
                // latitude) of the southwest corner; the second pair the northeast
                // corner", so a box whose west edge is east of its east edge wraps the
                // antimeridian.
                let within_latitude = position[1] >= *south && position[1] <= *north;
                let within_longitude = if west <= east {
                    position[0] >= *west && position[0] <= *east
                } else {
                    position[0] >= *west || position[0] <= *east
                };
                within_latitude && within_longitude
            }
            Self::Polygon(ring) => point_in_ring(position, ring),
        }
    }

    /// Whether the boundary is a geometry, which is what the documented matrix limits key
    /// on.
    fn is_geometry(&self) -> bool {
        !matches!(self, Self::Unbounded)
    }
}

/// Ray casting over a closed ring, in degrees.
fn point_in_ring(position: Position, ring: &[Position]) -> bool {
    let mut inside = false;
    for edge in ring.windows(2) {
        let (a, b) = (edge[0], edge[1]);
        let crosses = (a[1] > position[1]) != (b[1] > position[1]);
        if !crosses {
            continue;
        }
        let span = b[1] - a[1];
        if span.abs() < f64::EPSILON {
            continue;
        }
        let crossing_lon = a[0] + (position[1] - a[1]) / span * (b[0] - a[0]);
        if position[0] < crossing_lon {
            inside = !inside;
        }
    }
    inside
}

/// The boundary to compute within, and the boundary to report back.
///
/// `RoutingBoundary`'s documented default is `Unbounded` set to true. `AutoCircle` is a
/// request-only setting: the circle it asks for is derived from the submitted waypoints
/// here and reported as `Circle`, which is what "the response routing boundary will return
/// Circle derived from the AutoCircle settings" means.
fn resolve_boundary(
    input: &CalculateRouteMatrixInput,
    origins: &[Position],
    destinations: &[Position],
) -> Result<(Boundary, RouteMatrixBoundary), CalculateRouteMatrixError> {
    let Some(requested) = input.routing_boundary.as_ref() else {
        return Ok((Boundary::Unbounded, unbounded_boundary()));
    };
    let unbounded = requested.unbounded == Some(true);
    match (&requested.geometry, unbounded) {
        (Some(_), true) => Err(invalid(
            "RoutingBoundary",
            "RoutingBoundary must set either Geometry or Unbounded, not both".to_string(),
        )),
        // A boundary that sets no geometry and turns `Unbounded` off bounds the matrix by
        // nothing at all, so it is refused rather than answered as unbounded.
        (None, false) if requested.unbounded == Some(false) => Err(invalid(
            "RoutingBoundary",
            "RoutingBoundary must set Geometry when Unbounded is false".to_string(),
        )),
        (None, _) => Ok((Boundary::Unbounded, unbounded_boundary())),
        (Some(geometry), false) => resolve_geometry(geometry, origins, destinations),
    }
}

/// The documented default boundary.
fn unbounded_boundary() -> RouteMatrixBoundary {
    RouteMatrixBoundary {
        geometry: None,
        unbounded: Some(true),
    }
}

/// The one geometry a `RoutingBoundary.Geometry` sets, resolved to the area to compute
/// within and the geometry to report.
fn resolve_geometry(
    geometry: &RouteMatrixBoundaryGeometry,
    origins: &[Position],
    destinations: &[Position],
) -> Result<(Boundary, RouteMatrixBoundary), CalculateRouteMatrixError> {
    let set = usize::from(geometry.auto_circle.is_some())
        + usize::from(geometry.circle.is_some())
        + usize::from(geometry.bounding_box.is_some())
        + usize::from(geometry.polygon.is_some());
    if set != 1 {
        return Err(invalid(
            "RoutingBoundary.Geometry",
            format!(
                "RoutingBoundary.Geometry must set exactly one of AutoCircle, Circle, BoundingBox or Polygon; {set} were set"
            ),
        ));
    }

    if let Some(auto) = &geometry.auto_circle {
        let (margin, max_radius) = (auto.margin.max(0) as f64, auto.max_radius.max(0) as f64);
        if margin + max_radius > MAX_BOUNDARY_RADIUS_M {
            return Err(invalid(
                "RoutingBoundary.Geometry.AutoCircle",
                format!(
                    "the total of MaxRadius and Margin must be less than or equal to {MAX_BOUNDARY_RADIUS_M} meters; got {}",
                    margin + max_radius
                ),
            ));
        }
        let waypoints: Vec<Position> = origins.iter().chain(destinations.iter()).copied().collect();
        let center = centroid(&waypoints);
        // The circle that "best attempts to include most waypoints": the one around their
        // centroid that reaches the furthest of them, clamped to MaxRadius, plus Margin —
        // and never past the documented maximum radius of a routing boundary circle.
        let reach = waypoints
            .iter()
            .map(|waypoint| state::haversine_m(center, *waypoint))
            .fold(0.0_f64, f64::max);
        let radius_m = (if max_radius > 0.0 { reach.min(max_radius) } else { reach } + margin)
            .min(MAX_BOUNDARY_RADIUS_M);
        return Ok((
            Boundary::Circle { center, radius_m },
            RouteMatrixBoundary {
                geometry: Some(RouteMatrixBoundaryGeometry {
                    auto_circle: None,
                    circle: Some(Circle {
                        center: state::position_to_vec(center),
                        radius: radius_m,
                    }),
                    bounding_box: None,
                    polygon: None,
                }),
                unbounded: None,
            },
        ));
    }

    if let Some(circle) = &geometry.circle {
        let center = state::validate_position("RoutingBoundary.Geometry.Circle.Center", &circle.center)
            .map_err(|message| invalid("RoutingBoundary.Geometry.Circle.Center", message))?;
        if !circle.radius.is_finite() || circle.radius < 0.0 || circle.radius > MAX_BOUNDARY_RADIUS_M {
            return Err(invalid(
                "RoutingBoundary.Geometry.Circle.Radius",
                format!(
                    "Circle.Radius must be in [0, {MAX_BOUNDARY_RADIUS_M}] meters; got {}",
                    circle.radius
                ),
            ));
        }
        return Ok((
            Boundary::Circle {
                center,
                radius_m: circle.radius,
            },
            RouteMatrixBoundary {
                geometry: Some(RouteMatrixBoundaryGeometry {
                    auto_circle: None,
                    circle: Some(circle.clone()),
                    bounding_box: None,
                    polygon: None,
                }),
                unbounded: None,
            },
        ));
    }

    if let Some(bounding_box) = &geometry.bounding_box {
        let member = "RoutingBoundary.Geometry.BoundingBox";
        if bounding_box.len() != 4 {
            return Err(invalid(
                member,
                format!(
                    "BoundingBox must be [west, south, east, north]; got {} values",
                    bounding_box.len()
                ),
            ));
        }
        let south_west = state::validate_position(member, &bounding_box[..2]).map_err(|message| invalid(member, message))?;
        let north_east = state::validate_position(member, &bounding_box[2..]).map_err(|message| invalid(member, message))?;
        // The second pair is the north-east corner, so its latitude cannot be south of the
        // first pair's. A longitude that runs the other way is the antimeridian wrap the
        // containment test honours.
        if north_east[1] < south_west[1] {
            return Err(invalid(
                member,
                format!(
                    "BoundingBox is [west, south, east, north]: the north latitude cannot be south of the south one; got {} and {}",
                    south_west[1], north_east[1]
                ),
            ));
        }
        return Ok((
            Boundary::BoundingBox {
                west: south_west[0],
                south: south_west[1],
                east: north_east[0],
                north: north_east[1],
            },
            RouteMatrixBoundary {
                geometry: Some(RouteMatrixBoundaryGeometry {
                    auto_circle: None,
                    circle: None,
                    bounding_box: Some(bounding_box.clone()),
                    polygon: None,
                }),
                unbounded: None,
            },
        ));
    }

    let polygon = geometry
        .polygon
        .as_ref()
        .ok_or_else(|| internal("the routing boundary geometry was resolved to nothing"))?;
    let member = "RoutingBoundary.Geometry.Polygon";
    let ring = polygon
        .first()
        .ok_or_else(|| invalid(member, "Polygon must carry exactly one linear ring".to_string()))?;
    if ring.len() < 4 {
        return Err(invalid(
            member,
            format!(
                "a linear ring is a closed sequence of four or more positions; got {}",
                ring.len()
            ),
        ));
    }
    if ring.len() > MAX_RING_POSITIONS {
        return Err(invalid(
            member,
            format!(
                "a linear ring may carry at most {MAX_RING_POSITIONS} positions; got {}",
                ring.len()
            ),
        ));
    }
    let mut positions = Vec::with_capacity(ring.len());
    for (index, position) in ring.iter().enumerate() {
        let member = format!("{member}[0][{index}]");
        positions.push(state::validate_position(&member, position).map_err(|message| invalid(&member, message))?);
    }
    if positions.first() != positions.last() {
        return Err(invalid(
            member,
            "a linear ring must be closed: its first and last positions are the same".to_string(),
        ));
    }
    Ok((
        Boundary::Polygon(positions),
        RouteMatrixBoundary {
            geometry: Some(RouteMatrixBoundaryGeometry {
                auto_circle: None,
                circle: None,
                bounding_box: None,
                polygon: Some(polygon.clone()),
            }),
            unbounded: None,
        },
    ))
}

/// The mean of a set of positions. Empty is unreachable (the model pins both lists
/// non-empty) and answers the null island rather than a NaN.
fn centroid(positions: &[Position]) -> Position {
    if positions.is_empty() {
        return [0.0, 0.0];
    }
    let count = positions.len() as f64;
    [
        positions.iter().map(|position| position[0]).sum::<f64>() / count,
        positions.iter().map(|position| position[1]).sum::<f64>() / count,
    ]
}

/// The documented limits on how large a matrix may be, which depend on whether the
/// routing boundary is a geometry or unbounded.
fn check_matrix_size(
    boundary: &Boundary,
    origins: usize,
    destinations: usize,
) -> Result<(), CalculateRouteMatrixError> {
    let (max_origins, max_destinations, max_pairs) = if boundary.is_geometry() {
        GEOMETRY_LIMITS
    } else {
        UNBOUNDED_LIMITS
    };
    if origins > max_origins {
        return Err(invalid(
            "Origins",
            format!("at most {max_origins} origins are allowed for this routing boundary; got {origins}"),
        ));
    }
    if destinations > max_destinations {
        return Err(invalid(
            "Destinations",
            format!("at most {max_destinations} destinations are allowed for this routing boundary; got {destinations}"),
        ));
    }
    if origins.saturating_mul(destinations) > max_pairs {
        return Err(invalid(
            "Origins",
            format!(
                "the total matrix size must not exceed {max_pairs} for this routing boundary; got {}",
                origins.saturating_mul(destinations)
            ),
        ));
    }
    Ok(())
}
