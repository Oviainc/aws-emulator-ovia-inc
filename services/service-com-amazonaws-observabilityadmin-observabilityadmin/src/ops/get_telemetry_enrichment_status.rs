// SPDX-License-Identifier: CC0-1.0

//! @op GetTelemetryEnrichmentStatus
//!
//! HAND-AUTHORED, not scaffolded: the op-stub generator declined this operation
//! because its input is the Smithy `Unit` shape (`src/ops/SKIPPED_STUBS.md`), so
//! until now it was modeled-but-unwired and the framework's unroutable-operation
//! path answered it with an unmodeled `NotImplemented`. This is the ONLY file to
//! touch for this operation.
//!
//! ## What it does
//!
//! `@readonly`: it REPORTS the resource tags for telemetry feature of the region
//! this request was signed for, exactly as the last `StartTelemetryEnrichment` /
//! `StopTelemetryEnrichment` left it, and writes nothing.
//!
//! A region neither call has ever named has no such feature to report, which is what
//! the modelled 404 `ResourceNotFoundException` answers — the one modelled error of
//! this operation that is not a common error, and the only way a caller can tell
//! "never started" from `Stopped`.
//!
//! The managed view ARN is reported only while the feature is running, as
//! `AwsResourceExplorerManagedViewArn`'s own documentation qualifies it ("if the
//! feature is enabled"); the view a stopped feature would read again is still
//! recorded, so starting it again reports the same ARN.
//!
//! `TelemetryEnrichmentStatus` carries no transient value — only `Running`, `Stopped`
//! and `Impaired` — so this feature has no in-flight state: a start leaves it
//! running at once and a stop leaves it stopped at once. `Impaired` describes a
//! managed view this emulator cannot degrade, so it is never reported.
//!
//! Authorization and request-rate limiting are deliberately out of scope for this
//! emulator: the framework verifies every request signature, but no IAM principal is
//! evaluated and no throughput limit is enforced, so the modelled
//! `AccessDeniedException` is never answered by this read and
//! `TooManyRequestsException` is not either. Recorded omissions, not oversights.
//!
//! ---
//!
//! # Modeled contract for `GetTelemetryEnrichmentStatus`
//!
//! From this service's Smithy model. For the FULL text of any one item, ask the
//! model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-observabilityadmin-observabilityadmin/smithy-model/com-amazonaws-observabilityadmin-observabilityadmin.json")
//! ```
//!
//! Returns the current status of the resource tags for telemetry feature, which
//! enhances telemetry data with additional resource metadata from Resource Explorer.
//!
//! Route: `POST /GetTelemetryEnrichmentStatus -> 200`. `@readonly`.
//!
//! ## Input
//!
//! (no members — the input is `smithy.api#Unit`)
//!
//! ## Output
//!
//! * `Status` (TelemetryEnrichmentStatus) — The current status of the resource tags
//!   for telemetry feature (`Running`, `Stopped`, or `Impaired`).
//! * `AwsResourceExplorerManagedViewArn` (len 20..2048, pattern
//!   `^arn:aws([a-z0-9\-]+)?:resource-explorer-2:([a-z0-9\-]+)?:([0-9]{12})?:managed-view/(.+)$`)
//!   — The Amazon Resource Name (ARN) of the Resource Explorer managed view used for
//!   resource tags for telemetry, if the feature is enabled.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 400) — Indicates you don't have permissions to
//!   perform the requested operation.
//! * `InternalServerException` (HTTP 500) — Indicates the request has failed to
//!   process because of an unknown server error, exception, or failure.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource (such as a
//!   telemetry rule) could not be found.
//! * `TooManyRequestsException` (HTTP 429) — The request throughput limit was
//!   exceeded.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` and `#[smithy(..)]` below expand to invocations of the LOCAL
// declarative `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-observabilityadmin-observabilityadmin.json");

/// `GetTelemetryEnrichmentStatus`'s input: the Smithy `Unit` shape, so there is
/// nothing on the wire to read and nothing a caller can get wrong.
#[derive(Debug, Clone, Default)]
pub struct GetTelemetryEnrichmentStatusInput;

impl GetTelemetryEnrichmentStatusInput {
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "POST",
        "/GetTelemetryEnrichmentStatus",
        ::std::option::Option::None,
    ));
}

pub static GET_TELEMETRY_ENRICHMENT_STATUS_INPUT_SCHEMA: &::aws_smithy_schema::Schema =
    &GetTelemetryEnrichmentStatusInput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for GetTelemetryEnrichmentStatusInput {
    fn serialize_members(
        &self,
        _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for GetTelemetryEnrichmentStatusInput {
    fn deserialize(
        __d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        __d.read_struct(&Self::SCHEMA, &mut |_member, _d| Ok(()))?;
        Ok(Self)
    }
}

#[smithy_macros::smithy(
    shape = "com.amazonaws.observabilityadmin#GetTelemetryEnrichmentStatusOutput"
)]
#[derive(Debug, Clone, Default)]
pub struct GetTelemetryEnrichmentStatusOutput {
    pub status: Option<String>,
    pub aws_resource_explorer_managed_view_arn: Option<String>,
}

// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.observabilityadmin#GetTelemetryEnrichmentStatus")]
#[derive(Debug)]
pub enum GetTelemetryEnrichmentStatusErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_telemetry_enrichment_status::GetTelemetryEnrichmentStatusError`).
pub type GetTelemetryEnrichmentStatusError =
    crate::scaffolded::Scaffolded<GetTelemetryEnrichmentStatusErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<GetTelemetryEnrichmentStatusErrorModeled> for GetTelemetryEnrichmentStatusError {
    fn from(modeled: GetTelemetryEnrichmentStatusErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetTelemetryEnrichmentStatus` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetTelemetryEnrichmentStatus<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetTelemetryEnrichmentStatus<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)`.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetTelemetryEnrichmentStatus<Eng>
{
    type Input = GetTelemetryEnrichmentStatusInput;
    type Output = GetTelemetryEnrichmentStatusOutput;
    type Error = GetTelemetryEnrichmentStatusError;

    const NAME: &'static str = "GetTelemetryEnrichmentStatus";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        GET_TELEMETRY_ENRICHMENT_STATUS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        GET_TELEMETRY_ENRICHMENT_STATUS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = &input;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetTelemetryEnrichmentStatusErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("the telemetry enrichment store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;

        let region = crate::state::request_region(ctx.parts());

        let store = state.lock();
        let Some(record) = store.telemetry_enrichment(&region) else {
            return Err(
                GetTelemetryEnrichmentStatusErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(format!(
                            "the resource tags for telemetry feature has never been started in \
                             {region}"
                        )),
                        resource_type: Some("telemetry-enrichment".to_string()),
                        ..Default::default()
                    },
                )
                .into(),
            );
        };

        Ok(GetTelemetryEnrichmentStatusOutput {
            status: Some(record.status().to_string()),
            aws_resource_explorer_managed_view_arn: record.reported_view_arn(),
        })
    }
}
