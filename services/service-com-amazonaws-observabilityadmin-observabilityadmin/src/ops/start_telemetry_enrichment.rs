// SPDX-License-Identifier: CC0-1.0

//! @op StartTelemetryEnrichment
//!
//! HAND-AUTHORED, not scaffolded: the op-stub generator declined this operation
//! because its input is the Smithy `Unit` shape (`src/ops/SKIPPED_STUBS.md`), so
//! until now it was modeled-but-unwired and the framework's unroutable-operation
//! path answered it with an unmodeled `NotImplemented`. This is the ONLY file to
//! touch for this operation.
//!
//! ## What it does
//!
//! It MUTATES: enabling the resource tags for telemetry feature is what the call is
//! for. It records the feature of the region this request was signed for as running
//! and mints the Resource Explorer managed view it reads, then answers the modelled
//! 202 with that status and view — the same pair `GetTelemetryEnrichmentStatus`
//! reports from then on.
//!
//! `TelemetryEnrichmentStatus` carries no transient value (`Running`, `Stopped`,
//! `Impaired`), so the feature is running the moment this returns: there is no
//! state for the clock to move it out of.
//!
//! The view is minted once per region and kept across a stop, so a feature started,
//! stopped and started again reads the one view its first start created.
//!
//! Idempotent, so the modelled `ConflictException` is unreachable: starting an
//! already-running feature leaves it running on the one view it already has, which
//! is the state the caller asked for rather than a conflict with it.
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
//! # Modeled contract for `StartTelemetryEnrichment`
//!
//! From this service's Smithy model. For the FULL text of any one item, ask the
//! model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-observabilityadmin-observabilityadmin/smithy-model/com-amazonaws-observabilityadmin-observabilityadmin.json")
//! ```
//!
//! Enables the resource tags for telemetry feature for your account, which enhances
//! telemetry data with additional resource metadata from Resource Explorer to provide
//! richer context for monitoring and observability.
//!
//! Route: `POST /StartTelemetryEnrichment -> 202`.
//!
//! ## Input
//!
//! (no members — the input is `smithy.api#Unit`)
//!
//! ## Output
//!
//! * `Status` (TelemetryEnrichmentStatus) — The status of the resource tags for
//!   telemetry feature after the start operation (`Running`, `Stopped`, or
//!   `Impaired`).
//! * `AwsResourceExplorerManagedViewArn` (len 20..2048, pattern
//!   `^arn:aws([a-z0-9\-]+)?:resource-explorer-2:([a-z0-9\-]+)?:([0-9]{12})?:managed-view/(.+)$`)
//!   — The Amazon Resource Name (ARN) of the Resource Explorer managed view created
//!   for resource tags for telemetry.
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

/// `StartTelemetryEnrichment`'s input: the Smithy `Unit` shape, so there is nothing
/// on the wire to read and nothing a caller can get wrong.
#[derive(Debug, Clone, Default)]
pub struct StartTelemetryEnrichmentInput;

impl StartTelemetryEnrichmentInput {
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "POST",
        "/StartTelemetryEnrichment",
        ::std::option::Option::None,
    ));
}

pub static START_TELEMETRY_ENRICHMENT_INPUT_SCHEMA: &::aws_smithy_schema::Schema =
    &StartTelemetryEnrichmentInput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for StartTelemetryEnrichmentInput {
    fn serialize_members(
        &self,
        _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for StartTelemetryEnrichmentInput {
    fn deserialize(
        __d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        __d.read_struct(&Self::SCHEMA, &mut |_member, _d| Ok(()))?;
        Ok(Self)
    }
}

#[smithy_macros::smithy(shape = "com.amazonaws.observabilityadmin#StartTelemetryEnrichmentOutput")]
#[derive(Debug, Clone, Default)]
pub struct StartTelemetryEnrichmentOutput {
    pub status: Option<String>,
    pub aws_resource_explorer_managed_view_arn: Option<String>,
}

// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.observabilityadmin#StartTelemetryEnrichment")]
#[derive(Debug)]
pub enum StartTelemetryEnrichmentErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_telemetry_enrichment::StartTelemetryEnrichmentError`).
pub type StartTelemetryEnrichmentError =
    crate::scaffolded::Scaffolded<StartTelemetryEnrichmentErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<StartTelemetryEnrichmentErrorModeled> for StartTelemetryEnrichmentError {
    fn from(modeled: StartTelemetryEnrichmentErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartTelemetryEnrichment` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct StartTelemetryEnrichment<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartTelemetryEnrichment<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)`.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartTelemetryEnrichment<Eng>
{
    type Input = StartTelemetryEnrichmentInput;
    type Output = StartTelemetryEnrichmentOutput;
    type Error = StartTelemetryEnrichmentError;

    const NAME: &'static str = "StartTelemetryEnrichment";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        START_TELEMETRY_ENRICHMENT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        START_TELEMETRY_ENRICHMENT_OUTPUT_SCHEMA;
    // The model's own `@http` code for this operation: the work is accepted, and the
    // feature's status is what the caller reads back.
    const SUCCESS_STATUS: u16 = 202u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = &input;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            StartTelemetryEnrichmentErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("the telemetry enrichment store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;

        let region = crate::state::request_region(ctx.parts());
        let Some(record) = state.lock().start_telemetry_enrichment(&region) else {
            // The feature store is finite ([`crate::state::MAX_TELEMETRY_FEATURE_REGIONS`]):
            // a region past its cap is refused before a record is allocated, with the
            // modelled retryable error (this operation models no quota one).
            return Err(
                StartTelemetryEnrichmentErrorModeled::TooManyRequestsException(
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

        Ok(StartTelemetryEnrichmentOutput {
            status: Some(record.status().to_string()),
            aws_resource_explorer_managed_view_arn: record.reported_view_arn(),
        })
    }
}
