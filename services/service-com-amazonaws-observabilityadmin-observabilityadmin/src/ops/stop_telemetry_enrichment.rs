// SPDX-License-Identifier: CC0-1.0

//! @op StopTelemetryEnrichment
//!
//! HAND-AUTHORED, not scaffolded: the op-stub generator declined this operation
//! because its input is the Smithy `Unit` shape (`src/ops/SKIPPED_STUBS.md`), so
//! until now it was modeled-but-unwired and the framework's unroutable-operation
//! path answered it with an unmodeled `NotImplemented`. This is the ONLY file to
//! touch for this operation.
//!
//! ## What it does
//!
//! It MUTATES: disabling the resource tags for telemetry feature is what the call is
//! for. It records the feature of the region this request was signed for as stopped
//! and answers the modelled 202 with that status — the status
//! `GetTelemetryEnrichmentStatus` reports from then on.
//!
//! IDEMPOTENT, and the modelled `ConflictException` is therefore unreachable: the
//! feature has one off state, so a redundant stop leaves it exactly `Stopped` rather
//! than conflicting with the state the caller is asking for. A stop of a region no
//! start ever named records the stop too, so the status read reports `Stopped`
//! instead of the 404 a never-named region answers — the caller asked for the
//! feature to be off, and it is.
//!
//! The managed view a previous start minted is kept, so starting the feature again
//! reads the same view; `StopTelemetryEnrichmentOutput` has no ARN member to report
//! it with.
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
//! # Modeled contract for `StopTelemetryEnrichment`
//!
//! From this service's Smithy model. For the FULL text of any one item, ask the
//! model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-observabilityadmin-observabilityadmin/smithy-model/com-amazonaws-observabilityadmin-observabilityadmin.json")
//! ```
//!
//! Disables the resource tags for telemetry feature for your account, stopping the
//! enhancement of telemetry data with additional resource metadata.
//!
//! Route: `POST /StopTelemetryEnrichment -> 202`.
//!
//! ## Input
//!
//! (no members — the input is `smithy.api#Unit`)
//!
//! ## Output
//!
//! * `Status` (TelemetryEnrichmentStatus) — The status of the resource tags for
//!   telemetry feature after the stop operation (`Running`, `Stopped`, or
//!   `Impaired`).
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 400) — Indicates you don't have permissions to
//!   perform the requested operation.
//! * `ConflictException` (HTTP 409) — The requested operation conflicts with the
//!   current state of the specified resource or with another request.
//! * `InternalServerException` (HTTP 500) — Indicates the request has failed to
//!   process because of an unknown server error, exception, or failure.
//! * `TooManyRequestsException` (HTTP 429) — The request throughput limit was
//!   exceeded.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` and `#[smithy(..)]` below expand to invocations of the LOCAL
// declarative `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-observabilityadmin-observabilityadmin.json");

/// `StopTelemetryEnrichment`'s input: the Smithy `Unit` shape, so there is nothing on
/// the wire to read and nothing a caller can get wrong.
#[derive(Debug, Clone, Default)]
pub struct StopTelemetryEnrichmentInput;

impl StopTelemetryEnrichmentInput {
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "POST",
        "/StopTelemetryEnrichment",
        ::std::option::Option::None,
    ));
}

pub static STOP_TELEMETRY_ENRICHMENT_INPUT_SCHEMA: &::aws_smithy_schema::Schema =
    &StopTelemetryEnrichmentInput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for StopTelemetryEnrichmentInput {
    fn serialize_members(
        &self,
        _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for StopTelemetryEnrichmentInput {
    fn deserialize(
        __d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        __d.read_struct(&Self::SCHEMA, &mut |_member, _d| Ok(()))?;
        Ok(Self)
    }
}

#[smithy_macros::smithy(shape = "com.amazonaws.observabilityadmin#StopTelemetryEnrichmentOutput")]
#[derive(Debug, Clone, Default)]
pub struct StopTelemetryEnrichmentOutput {
    pub status: Option<String>,
}

// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.observabilityadmin#StopTelemetryEnrichment")]
#[derive(Debug)]
pub enum StopTelemetryEnrichmentErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::stop_telemetry_enrichment::StopTelemetryEnrichmentError`).
pub type StopTelemetryEnrichmentError =
    crate::scaffolded::Scaffolded<StopTelemetryEnrichmentErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<StopTelemetryEnrichmentErrorModeled> for StopTelemetryEnrichmentError {
    fn from(modeled: StopTelemetryEnrichmentErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StopTelemetryEnrichment` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct StopTelemetryEnrichment<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StopTelemetryEnrichment<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)`.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StopTelemetryEnrichment<Eng>
{
    type Input = StopTelemetryEnrichmentInput;
    type Output = StopTelemetryEnrichmentOutput;
    type Error = StopTelemetryEnrichmentError;

    const NAME: &'static str = "StopTelemetryEnrichment";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        STOP_TELEMETRY_ENRICHMENT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        STOP_TELEMETRY_ENRICHMENT_OUTPUT_SCHEMA;
    // The model's own `@http` code for this operation.
    const SUCCESS_STATUS: u16 = 202u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = &input;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            StopTelemetryEnrichmentErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("the telemetry enrichment store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;

        let region = crate::state::request_region(ctx.parts());
        let Some(record) = state.lock().stop_telemetry_enrichment(&region) else {
            // The feature store is finite ([`crate::state::MAX_TELEMETRY_FEATURE_REGIONS`]):
            // a region past its cap is refused before a record is allocated, with the
            // modelled retryable error (this operation models no quota one).
            return Err(
                StopTelemetryEnrichmentErrorModeled::TooManyRequestsException(
                    TooManyRequestsException {
                        message: Some(format!(
                            "this emulator holds the resource tags for telemetry feature of at \
                             most {} regions",
                            crate::state::MAX_TELEMETRY_FEATURE_REGIONS
                        )),
                    },
                )
                .into(),
            );
        };

        Ok(StopTelemetryEnrichmentOutput {
            status: Some(record.status().to_string()),
        })
    }
}
