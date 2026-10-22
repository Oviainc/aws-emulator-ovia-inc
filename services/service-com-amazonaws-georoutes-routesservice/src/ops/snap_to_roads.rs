// SPDX-License-Identifier: CC0-1.0

//! @op SnapToRoads
//!
//! Each trace point is snapped onto the nearest road of the network in [`crate::state`]
//! that the requested travel mode may use. The answer carries one snapped point per
//! submitted trace point, at the same index, with the submitted position echoed as
//! `OriginalPosition`, the point on the road as `SnappedPosition` and a `Confidence` that
//! falls off with the snap offset over `SnapRadius`. A point whose nearest usable road is
//! further away than `SnapRadius` is not matched: it keeps its submitted position, scores
//! zero confidence, and its index is listed under a `TracePointsNotMatched` notice.
//!
//! `SnappedGeometry` is the line through the positions that DID match a road; a point
//! reported unmatched is left out of it, so the geometry and the per-point confidence
//! cannot disagree, and a trace with fewer than two matched points reports no geometry at
//! all.
//!
//! `TracePoints[].Timestamp`, `Speed` and `Heading` are read but have nothing to act on:
//! the network carries no one-way or direction data for a heading to disambiguate and no
//! time-varying road data for a timestamp or a speed to select, so the snap of a point
//! depends on its position and the travel mode alone. `TravelModeOptions` is the same —
//! the model carries no vehicle restriction for a truck's dimensions or cargo to meet.
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
//! # Modeled contract for `SnapToRoads`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-georoutes-routesservice/smithy-model/com-amazonaws-georoutes-routesservice.json")
//! ```
//!
//! SnapToRoads matches GPS trace to roads most likely traveled on. For more information,
//! see Snap to Roads in the Amazon Location Service Developer Guide.
//!
//! ## Input
//!
//! * `Key` (ApiKey, len ..1000) — Optional: The API key to be used for authorization. Either
//!   an API key or valid SigV4 signature must be provided when making a request.
//! * `SnapRadius` (DistanceMeters, range 0..4294967295, default 0) **required** — The radius
//!   around the provided tracepoint that is considered for snapping. Unit: meters Default
//!   value: 300
//! * `SnappedGeometryFormat` (GeometryFormat, one of: FlexiblePolyline | Simple) — Chooses
//!   what the returned SnappedGeometry format should be. Default value: FlexiblePolyline
//! * `TracePoints` (RoadSnapTracePointList) **required** — List of trace points to be snapped
//!   onto the road network.
//! * `TravelMode` (RoadSnapTravelMode, one of: Car | Pedestrian | Scooter | Truck) —
//!   Specifies the mode of transport when calculating a route. Used in estimating the speed
//!   of travel and road compatibility. Default value: Car
//! * `TravelModeOptions` (RoadSnapTravelModeOptions) — Travel mode related options for the
//!   provided travel mode.
//!
//! ## Output
//!
//! * `Notices` (RoadSnapNoticeList) **required** — Notices are additional information
//!   returned that indicate issues that occurred during route calculation.
//! * `PricingBucket` (String) **required** — The pricing bucket for which the query is
//!   charged at.
//! * `SnappedGeometry` (RoadSnapSnappedGeometry) — The interpolated geometry for the snapped
//!   route onto the road network.
//! * `SnappedGeometryFormat` (GeometryFormat, one of: FlexiblePolyline | Simple) **required**
//!   — Specifies the format of the geometry returned for each leg of the route.
//! * `SnappedTracePoints` (RoadSnapSnappedTracePointList) **required** — The trace points
//!   snapped onto the road network.
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

use crate::state::{self, Position, TravelMode};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `SnapToRoadsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-georoutes-routesservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `SnapToRoadsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.georoutes#SnapToRoads")]
#[derive(Debug)]
pub enum SnapToRoadsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::snap_to_roads::SnapToRoadsError`). The wrapper is kept because it is
/// also the only place a request this operation models no error for could be refused;
/// every refusal below uses the model's own `ValidationException`.
pub type SnapToRoadsError = crate::scaffolded::Scaffolded<SnapToRoadsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<SnapToRoadsErrorModeled> for SnapToRoadsError {
    fn from(modeled: SnapToRoadsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `SnapToRoads` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct SnapToRoads<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> SnapToRoads<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for SnapToRoads<Eng>
{
    type Input = SnapToRoadsInput;
    type Output = SnapToRoadsOutput;
    type Error = SnapToRoadsError;

    const NAME: &'static str = "SnapToRoads";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SNAP_TO_ROADS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SNAP_TO_ROADS_OUTPUT_SCHEMA;
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
            Some(value) => TravelMode::from_model_values(value, ROAD_SNAP_TRAVEL_MODE_VALUES).ok_or_else(|| {
                invalid(
                    "TravelMode",
                    format!("TravelMode {value} is not a RoadSnapTravelMode"),
                )
            })?,
        };
        let geometry_format = state::geometry_format("SnappedGeometryFormat", input.snapped_geometry_format.as_ref())
            .map_err(|message| invalid("SnappedGeometryFormat", message))?;

        // The model's `@default` is 0, which is no radius at all; the documented default
        // the service applies is 300 metres. The modeled range (`0..10000`) is not
        // enforced on a defaulted member, so the handler refuses anything outside it
        // rather than reading it as "unset".
        if !(0..=state::MAX_SNAP_RADIUS_M).contains(&input.snap_radius) {
            return Err(invalid(
                "SnapRadius",
                format!(
                    "SnapRadius must be in [0, {}] metres; got {}",
                    state::MAX_SNAP_RADIUS_M, input.snap_radius
                ),
            ));
        }
        let snap_radius_m = if input.snap_radius == 0 {
            state::DEFAULT_SNAP_RADIUS_M as f64
        } else {
            input.snap_radius as f64
        };

        // The documented list bounds ("List of trace points to be snapped onto the road
        // network", 2..5000 on `TracePoints`). They are enforced HERE rather than relied on
        // from the boundary validator, so the per-request work below is bounded whatever
        // the generated validator does with the member's length.
        if !(MIN_TRACE_POINTS..=MAX_TRACE_POINTS).contains(&input.trace_points.len()) {
            return Err(invalid(
                "TracePoints",
                format!(
                    "TracePoints must carry between {MIN_TRACE_POINTS} and {MAX_TRACE_POINTS} points; got {}",
                    input.trace_points.len()
                ),
            ));
        }
        let mut positions = Vec::with_capacity(input.trace_points.len());
        for (index, trace_point) in input.trace_points.iter().enumerate() {
            let member = format!("TracePoints[{index}].Position");
            positions.push(
                state::validate_position(&member, &trace_point.position)
                    .map_err(|message| invalid(&member, message))?,
            );
        }

        let state_handle =
            state::service_state(&*self.engine).ok_or_else(|| internal("the routes service state is unavailable"))?;

        // Snapping runs in chunks: the lock on the shared state is taken per chunk and
        // released between chunks, where the handler yields — so a long trace neither
        // holds the lock for the whole request nor leaves a client's cancellation or the
        // server's deadline nowhere to land.
        let mut snapped: Vec<Option<state::SnappedPosition>> = Vec::with_capacity(positions.len());
        for chunk in positions.chunks(SNAP_CHUNK) {
            {
                let mut state = state_handle.lock();
                snapped.extend(chunk.iter().map(|position| state.snap(*position, travel_mode)));
            }
            tokio::task::yield_now().await;
        }
        let pricing_bucket = state_handle.lock().pricing_bucket.clone();

        let mut snapped_trace_points = Vec::with_capacity(positions.len());
        let mut snapped_path: Vec<Position> = Vec::with_capacity(positions.len());
        let mut unmatched: Vec<i32> = Vec::new();
        for ((index, position), candidate) in positions.iter().enumerate().zip(snapped) {
            // Within the radius the point matches a road; beyond it there is no candidate
            // to match, so the point is reported unmatched rather than moved.
            let matched = candidate.filter(|candidate| candidate.offset_m <= snap_radius_m);
            match matched {
                Some(candidate) => {
                    snapped_path.push(candidate.position);
                    snapped_trace_points.push(RoadSnapSnappedTracePoint {
                        // Confidence falls off linearly with how far the point had to
                        // move, over the radius the caller allowed: on the road is 1, at
                        // the edge of the radius is 0.
                        confidence: (1.0 - candidate.offset_m / snap_radius_m).clamp(0.0, 1.0),
                        original_position: state::position_to_vec(*position),
                        snapped_position: state::position_to_vec(candidate.position),
                    });
                }
                None => {
                    if let Ok(index) = i32::try_from(index) {
                        unmatched.push(index);
                    }
                    // An unmatched point is NOT on the snapped route, so it is left out of
                    // the snapped geometry: the line would otherwise run through a
                    // position the same answer reports as matching no road.
                    snapped_trace_points.push(RoadSnapSnappedTracePoint {
                        confidence: 0.0,
                        original_position: state::position_to_vec(*position),
                        snapped_position: state::position_to_vec(*position),
                    });
                }
            }
        }

        let mut notices = Vec::new();
        if !unmatched.is_empty() {
            notices.push(RoadSnapNotice {
                code: "TracePointsNotMatched".to_string(),
                title: format!(
                    "{} trace points had no road within the {snap_radius_m} metre snap radius",
                    unmatched.len()
                ),
                trace_point_indexes: unmatched,
            });
        }

        let snapped_geometry = snapped_geometry(&snapped_path, &geometry_format)?;
        Ok(SnapToRoadsOutput {
            notices,
            pricing_bucket,
            snapped_geometry,
            snapped_geometry_format: geometry_format,
            snapped_trace_points,
        })
    }
}

/// The bounds of the `TracePoints` list. The Amazon Location Service API reference states
/// for `SnapToRoads` request parameter `TracePoints`: "Array Members: Minimum number of 2
/// items. Maximum number of 5000 items." — a trace is a sequence of positions travelled
/// through, so a single point is not one. This crate's Smithy model omits the `@length`
/// trait on `RoadSnapTracePointList`, so the framework's constraint walk does not enforce
/// it and this handler does; the upper bound also bounds the work one request can ask for.
const MIN_TRACE_POINTS: usize = 2;
const MAX_TRACE_POINTS: usize = 5_000;

/// How many trace points are snapped under one acquisition of the shared state lock.
const SNAP_CHUNK: usize = 256;

/// The interpolated geometry of the snapped route, in the requested format. `None` when
/// there is no line to draw — a `LineString` carries two positions at least.
fn snapped_geometry(path: &[Position], format: &str) -> Result<Option<RoadSnapSnappedGeometry>, SnapToRoadsError> {
    if path.len() < 2 {
        return Ok(None);
    }
    if format == "Simple" {
        return Ok(Some(RoadSnapSnappedGeometry {
            line_string: Some(state::line_string(path)),
            polyline: None,
        }));
    }
    Ok(Some(RoadSnapSnappedGeometry {
        line_string: None,
        polyline: Some(
            state::encode_flexible_polyline(path)
                .ok_or_else(|| internal("the snapped geometry could not be encoded as a Flexible Polyline"))?,
        ),
    }))
}

/// The model's own `ValidationException` (HTTP 400) for a request member this service
/// refuses, naming the member and the rule it broke.
fn invalid(member: &str, message: String) -> SnapToRoadsError {
    SnapToRoadsErrorModeled::ValidationException(ValidationException {
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
fn internal(message: &str) -> SnapToRoadsError {
    SnapToRoadsErrorModeled::InternalServerException(InternalServerException {
        message: message.to_string(),
    })
    .into()
}
