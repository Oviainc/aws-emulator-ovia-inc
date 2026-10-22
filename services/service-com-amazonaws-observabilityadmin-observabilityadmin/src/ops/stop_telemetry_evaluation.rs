// SPDX-License-Identifier: CC0-1.0

//! @op StopTelemetryEvaluation
//!
//! HAND-AUTHORED, not scaffolded: the op-stub generator declined this operation
//! because its input is the Smithy `Unit` shape (`src/ops/SKIPPED_STUBS.md`), so
//! until now it was modeled-but-unwired and the framework's unroutable-operation
//! path answered it with an unmodeled `NotImplemented`. This is the ONLY file to
//! touch for this operation.
//!
//! ## What it does
//!
//! It MUTATES: offboarding the caller's account from the telemetry-config feature is
//! what this call is for. It records the offboarding of the region the request was
//! signed for, so `GetTelemetryEvaluationStatus` reports `STOPPING` until it comes
//! due ([`crate::state::EVALUATION_TRANSITION_DELAY_MS`]) and `STOPPED` from then
//! on. A stop signed for the home region of a multi-region configuration offboards
//! that configuration's spoke regions with it — the home region is "the single
//! management point for all multi-region operations on this account" — while a stop
//! signed for a spoke region offboards that region alone.
//!
//! The call is idempotent: repeating it is the same offboarding, and stopping a
//! region nothing ever started still records the offboarding the caller asked for.
//!
//! The modelled `ValidationException` is unreachable here: the input is
//! `smithy.api#Unit`, so there is nothing for a caller to get wrong.
//!
//! Authorization is deliberately out of scope for this emulator: the framework
//! verifies every request signature, but no IAM principal is evaluated, so the
//! modelled `AccessDeniedException` is never answered. The modelled
//! `TooManyRequestsException` is answered for one reason only — the emulator's own
//! finite feature store ([`crate::state::MAX_TELEMETRY_FEATURE_REGIONS`]) — and never
//! as a request-rate limit, which this emulator does not enforce.
//!
//! ---
//!
//! # Modeled contract for `StopTelemetryEvaluation`
//!
//! From this service's Smithy model. For the FULL text of any one item, ask the
//! model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-observabilityadmin-observabilityadmin/smithy-model/com-amazonaws-observabilityadmin-observabilityadmin.json")
//! ```
//!
//! This action begins offboarding the caller Amazon Web Services account from the
//! telemetry config feature.
//!
//! Route: `POST /StopTelemetryEvaluation -> 200`.
//!
//! ## Input
//!
//! (no members — the input is `smithy.api#Unit`)
//!
//! ## Output
//!
//! (no members — the output is `smithy.api#Unit`)
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 400) — Indicates you don't have permissions to
//!   perform the requested operation.
//! * `InternalServerException` (HTTP 500) — Indicates the request has failed to
//!   process because of an unknown server error, exception, or failure.
//! * `TooManyRequestsException` (HTTP 429) — The request throughput limit was
//!   exceeded.
//! * `ValidationException` (HTTP 400) — Indicates input validation failed. Check your
//!   request parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-observabilityadmin-observabilityadmin.json");

/// `StopTelemetryEvaluation`'s input: the Smithy `Unit` shape, so there is nothing
/// on the wire to read and nothing a caller can get wrong. The schema and both serde
/// impls are written out here because `smithy.api#Unit` is not a shape this service's
/// model declares, and the centralized walk still deserializes and validates every
/// input against `INPUT_SCHEMA` before `handle` runs.
#[derive(Debug, Clone, Default)]
pub struct StopTelemetryEvaluationInput;

impl StopTelemetryEvaluationInput {
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "POST",
        "/StopTelemetryEvaluation",
        ::std::option::Option::None,
    ));
}

pub static STOP_TELEMETRY_EVALUATION_INPUT_SCHEMA: &::aws_smithy_schema::Schema =
    &StopTelemetryEvaluationInput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for StopTelemetryEvaluationInput {
    fn serialize_members(
        &self,
        _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for StopTelemetryEvaluationInput {
    fn deserialize(
        __d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        __d.read_struct(&Self::SCHEMA, &mut |_member, _d| Ok(()))?;
        Ok(Self)
    }
}

/// `StopTelemetryEvaluation`'s output: the Smithy `Unit` shape, so the 200 carries an
/// empty body.
#[derive(Debug, Clone, Default)]
pub struct StopTelemetryEvaluationOutput;

impl StopTelemetryEvaluationOutput {
    // `with_http` carries this operation's modelled route, exactly as the scaffold
    // writes a `Unit` output elsewhere in this crate.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "POST",
        "/StopTelemetryEvaluation",
        ::std::option::Option::None,
    ));
}

pub static STOP_TELEMETRY_EVALUATION_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema =
    &StopTelemetryEvaluationOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for StopTelemetryEvaluationOutput {
    fn serialize_members(
        &self,
        _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        Ok(())
    }
}

// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.observabilityadmin#StopTelemetryEvaluation")]
#[derive(Debug)]
pub enum StopTelemetryEvaluationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    TooManyRequestsException(TooManyRequestsException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::stop_telemetry_evaluation::StopTelemetryEvaluationError`).
pub type StopTelemetryEvaluationError =
    crate::scaffolded::Scaffolded<StopTelemetryEvaluationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<StopTelemetryEvaluationErrorModeled> for StopTelemetryEvaluationError {
    fn from(modeled: StopTelemetryEvaluationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StopTelemetryEvaluation` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct StopTelemetryEvaluation<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StopTelemetryEvaluation<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)`.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StopTelemetryEvaluation<Eng>
{
    type Input = StopTelemetryEvaluationInput;
    type Output = StopTelemetryEvaluationOutput;
    type Error = StopTelemetryEvaluationError;

    const NAME: &'static str = "StopTelemetryEvaluation";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = STOP_TELEMETRY_EVALUATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        STOP_TELEMETRY_EVALUATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = &input;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            StopTelemetryEvaluationErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("the telemetry evaluation store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;

        let region = crate::state::request_region(ctx.parts());
        let now = crate::state::now_epoch_millis();
        if !state.lock().stop_telemetry_evaluation(
            crate::state::EvaluationScope::Account,
            &region,
            now,
        ) {
            // The feature store is finite ([`crate::state::MAX_TELEMETRY_FEATURE_REGIONS`]):
            // an offboarding that would have to record a region past its cap is
            // refused before a record is allocated, with the modelled retryable
            // error (this operation models no quota one).
            return Err(
                StopTelemetryEvaluationErrorModeled::TooManyRequestsException(
                    TooManyRequestsException {
                        message: Some(format!(
                            "this emulator holds the telemetry-config feature of at most {} \
                             regions",
                            crate::state::MAX_TELEMETRY_FEATURE_REGIONS
                        )),
                    },
                )
                .into(),
            );
        }

        Ok(StopTelemetryEvaluationOutput)
    }
}
