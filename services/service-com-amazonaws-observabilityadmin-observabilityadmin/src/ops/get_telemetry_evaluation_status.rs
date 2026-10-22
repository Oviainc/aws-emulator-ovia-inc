// SPDX-License-Identifier: CC0-1.0

//! @op GetTelemetryEvaluationStatus
//!
//! HAND-AUTHORED, not scaffolded: the op-stub generator declined this operation
//! because its input is the Smithy `Unit` shape (`src/ops/SKIPPED_STUBS.md`), so
//! until now it was modeled-but-unwired and the framework's unroutable-operation
//! path answered it with an unmodeled `NotImplemented`. This is the ONLY file to
//! touch for this operation.
//!
//! ## What it does
//!
//! `@readonly`: it REPORTS the onboarding status of the telemetry-config feature of
//! the region this request was signed for, exactly as the last
//! `StartTelemetryEvaluation` / `StopTelemetryEvaluation` of that region left it,
//! and writes nothing. A region no `Start` or `Stop` has ever named has never been
//! onboarded, and reports `NOT_STARTED`.
//!
//! The transient-to-terminal moves (`STARTING` -> `RUNNING`, `STOPPING` ->
//! `STOPPED`) are the clock's, not a write's: the stored record carries the instant
//! the transition comes due, and the status is computed from it on every read — so
//! two reads either side of that instant differ while nothing was written in
//! between, and a read never advances anything.
//!
//! `FailureReason` is never populated here: its own documentation says it "will only
//! be populated if `Status` is `FAILED_START` or `FAILED_STOP`", and nothing in this
//! emulator can fail to onboard or offboard a region.
//!
//! Authorization and request-rate limiting are deliberately out of scope for this
//! emulator: the framework verifies every request signature, but no IAM principal is
//! evaluated and no throughput limit is enforced, so the modelled
//! `AccessDeniedException` and `TooManyRequestsException` are never answered by this
//! read. That is a recorded omission, not an oversight.
//!
//! ---
//!
//! # Modeled contract for `GetTelemetryEvaluationStatus`
//!
//! From this service's Smithy model. For the FULL text of any one item, ask the
//! model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-observabilityadmin-observabilityadmin/smithy-model/com-amazonaws-observabilityadmin-observabilityadmin.json")
//! ```
//!
//! Returns the current onboarding status of the telemetry config feature, including
//! the status of the feature and reason the feature failed to start or stop.
//!
//! Route: `POST /GetTelemetryEvaluationStatus -> 200`. `@readonly`.
//!
//! ## Input
//!
//! (no members — the input is `smithy.api#Unit`)
//!
//! ## Output
//!
//! * `Status` (Status) — The onboarding status of the telemetry config feature.
//! * `FailureReason` (FailureReason) — Describes the reason for the failure status.
//!   The field will only be populated if `Status` is `FAILED_START` or `FAILED_STOP`.
//! * `HomeRegion` (Region, len 1..) — The Amazon Web Services Region that is
//!   designated as the home region for multi-region telemetry evaluation. The home
//!   region is the single management point for all multi-region operations on this
//!   account. This field is only present when multi-region telemetry evaluation is
//!   active.
//! * `RegionStatuses` (RegionStatuses) — A list of per-region telemetry evaluation
//!   statuses. Each entry indicates the evaluation status for a specific spoke region
//!   included in the multi-region configuration. This field is only present when
//!   multi-region telemetry evaluation is active.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 400) — Indicates you don't have permissions to
//!   perform the requested operation.
//! * `InternalServerException` (HTTP 500) — Indicates the request has failed to
//!   process because of an unknown server error, exception, or failure.
//! * `TooManyRequestsException` (HTTP 429) — The request throughput limit was
//!   exceeded.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` and `#[smithy(..)]` below expand to invocations of the LOCAL
// declarative `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-observabilityadmin-observabilityadmin.json");

/// `GetTelemetryEvaluationStatus`'s input: the Smithy `Unit` shape, so there is
/// nothing on the wire to read and nothing a caller can get wrong.
///
/// The empty shape still needs a schema and both serde impls, because the
/// framework's centralized request walk deserializes and validates every input
/// against `INPUT_SCHEMA` before `handle` runs. They are written out here rather
/// than generated because `smithy.api#Unit` is not a shape this service's model
/// declares. `with_http` carries the operation's method and URI, which is what the
/// REST request path reads to fill `@httpLabel` placeholders (there are none here);
/// the status slot is `None` on an INPUT schema, because the success status is an
/// operation fact and lives on `Operation::SUCCESS_STATUS`.
#[derive(Debug, Clone, Default)]
pub struct GetTelemetryEvaluationStatusInput;

impl GetTelemetryEvaluationStatusInput {
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "POST",
        "/GetTelemetryEvaluationStatus",
        ::std::option::Option::None,
    ));
}

pub static GET_TELEMETRY_EVALUATION_STATUS_INPUT_SCHEMA: &::aws_smithy_schema::Schema =
    &GetTelemetryEvaluationStatusInput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for GetTelemetryEvaluationStatusInput {
    fn serialize_members(
        &self,
        _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for GetTelemetryEvaluationStatusInput {
    fn deserialize(
        __d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        // The same walk a generated member-less input runs: read the struct and
        // ignore every member the protocol offers, because the shape declares none.
        __d.read_struct(&Self::SCHEMA, &mut |_member, _d| Ok(()))?;
        Ok(Self)
    }
}

#[smithy_macros::smithy(
    shape = "com.amazonaws.observabilityadmin#GetTelemetryEvaluationStatusOutput"
)]
#[derive(Debug, Clone, Default)]
pub struct GetTelemetryEvaluationStatusOutput {
    pub status: Option<String>,
    pub failure_reason: Option<String>,
    pub home_region: Option<String>,
    pub region_statuses: Option<Vec<RegionStatus>>,
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetTelemetryEvaluationStatusError` alias below.
#[smithy_macros::operation_error("com.amazonaws.observabilityadmin#GetTelemetryEvaluationStatus")]
#[derive(Debug)]
pub enum GetTelemetryEvaluationStatusErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_telemetry_evaluation_status::GetTelemetryEvaluationStatusError`).
pub type GetTelemetryEvaluationStatusError =
    crate::scaffolded::Scaffolded<GetTelemetryEvaluationStatusErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the
/// handler writes `Err(SomeModeledError.into())` rather than naming the wrapper
/// variant.
impl From<GetTelemetryEvaluationStatusErrorModeled> for GetTelemetryEvaluationStatusError {
    fn from(modeled: GetTelemetryEvaluationStatusErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetTelemetryEvaluationStatus` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetTelemetryEvaluationStatus<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetTelemetryEvaluationStatus<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)`.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetTelemetryEvaluationStatus<Eng>
{
    type Input = GetTelemetryEvaluationStatusInput;
    type Output = GetTelemetryEvaluationStatusOutput;
    type Error = GetTelemetryEvaluationStatusError;

    const NAME: &'static str = "GetTelemetryEvaluationStatus";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        GET_TELEMETRY_EVALUATION_STATUS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        GET_TELEMETRY_EVALUATION_STATUS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = &input;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetTelemetryEvaluationStatusErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("the telemetry evaluation store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;

        let region = crate::state::request_region(ctx.parts());
        let now = crate::state::now_epoch_millis();
        let scope = crate::state::EvaluationScope::Account;

        let store = state.lock();
        let Some(record) = store.telemetry_evaluation(scope, &region) else {
            // Never onboarded in this region. There is no modelled error for it —
            // `NOT_STARTED` IS the answer the `Status` enum carries for it, and the
            // multi-region members stay absent because no configuration exists.
            return Ok(GetTelemetryEvaluationStatusOutput {
                status: Some(crate::state::STATUS_NOT_STARTED.to_string()),
                ..Default::default()
            });
        };

        Ok(GetTelemetryEvaluationStatusOutput {
            status: Some(record.status_at(now).to_string()),
            // Only populated in the two FAILED_* statuses, which this emulator never
            // reports.
            failure_reason: None,
            // Both multi-region members are "only present when multi-region
            // telemetry evaluation is active".
            home_region: record
                .is_multi_region()
                .then(|| record.home_region.clone()),
            region_statuses: store.evaluation_region_statuses(scope, record, now),
        })
    }
}
