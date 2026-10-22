// SPDX-License-Identifier: CC0-1.0

//! @op GetTelemetryEvaluationStatusForOrganization
//!
//! HAND-AUTHORED, not scaffolded: the op-stub generator declined this operation
//! because its input is the Smithy `Unit` shape (`src/ops/SKIPPED_STUBS.md`), so
//! until now it was modeled-but-unwired and the framework's unroutable-operation
//! path answered it with an unmodeled `NotImplemented`. This is the ONLY file to
//! touch for this operation.
//!
//! ## What it does
//!
//! The organization-scoped twin of `GetTelemetryEvaluationStatus`: `@readonly`, it
//! REPORTS the onboarding status of the ORGANIZATION's telemetry-config feature in
//! the region this request was signed for, exactly as the last
//! `StartTelemetryEvaluationForOrganization` / `StopTelemetryEvaluationForOrganization`
//! left it, and writes nothing. The organization's feature is a separate record from
//! the caller's own account feature
//! ([`crate::state::EvaluationScope`]), so onboarding one does not report the other
//! as onboarded.
//!
//! A region no organization-scoped `Start` or `Stop` has ever named reports
//! `NOT_STARTED`; the transient-to-terminal moves are the clock's, computed from the
//! instant the transition came due, so a read never writes.
//!
//! Authorization and request-rate limiting are deliberately out of scope for this
//! emulator: the framework verifies every request signature, but no IAM principal is
//! evaluated and no throughput limit is enforced, so the modelled
//! `AccessDeniedException` and `TooManyRequestsException` are never answered by this
//! read — nor is the management-account restriction this operation's documentation
//! states, since this emulator has one account. Recorded omissions, not oversights.
//!
//! ---
//!
//! # Modeled contract for `GetTelemetryEvaluationStatusForOrganization`
//!
//! From this service's Smithy model. For the FULL text of any one item, ask the
//! model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-observabilityadmin-observabilityadmin/smithy-model/com-amazonaws-observabilityadmin-observabilityadmin.json")
//! ```
//!
//! This returns the onboarding status of the telemetry configuration feature for the
//! organization. It can only be called by a Management Account of an Amazon Web
//! Services Organization or an assigned Delegated Admin Account of Amazon CloudWatch
//! telemetry config.
//!
//! Route: `POST /GetTelemetryEvaluationStatusForOrganization -> 200`. `@readonly`.
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
//! * `HomeRegion` (Region, len 1..) — The home region for multi-region telemetry
//!   evaluation. Only present when multi-region telemetry evaluation is active.
//! * `RegionStatuses` (RegionStatuses) — A list of per-region telemetry evaluation
//!   statuses, one per spoke region of the multi-region configuration. Only present
//!   when multi-region telemetry evaluation is active.
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

// `#[operation_error]` and `#[smithy(..)]` below expand to invocations of the LOCAL
// declarative `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-observabilityadmin-observabilityadmin.json");

/// `GetTelemetryEvaluationStatusForOrganization`'s input: the Smithy `Unit` shape,
/// so there is nothing on the wire to read and nothing a caller can get wrong. The
/// schema and both serde impls are written out here because `smithy.api#Unit` is not
/// a shape this service's model declares.
#[derive(Debug, Clone, Default)]
pub struct GetTelemetryEvaluationStatusForOrganizationInput;

impl GetTelemetryEvaluationStatusForOrganizationInput {
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "POST",
        "/GetTelemetryEvaluationStatusForOrganization",
        ::std::option::Option::None,
    ));
}

pub static GET_TELEMETRY_EVALUATION_STATUS_FOR_ORGANIZATION_INPUT_SCHEMA:
    &::aws_smithy_schema::Schema = &GetTelemetryEvaluationStatusForOrganizationInput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct
    for GetTelemetryEvaluationStatusForOrganizationInput
{
    fn serialize_members(
        &self,
        _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct
    for GetTelemetryEvaluationStatusForOrganizationInput
{
    fn deserialize(
        __d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        __d.read_struct(&Self::SCHEMA, &mut |_member, _d| Ok(()))?;
        Ok(Self)
    }
}

#[smithy_macros::smithy(
    shape = "com.amazonaws.observabilityadmin#GetTelemetryEvaluationStatusForOrganizationOutput"
)]
#[derive(Debug, Clone, Default)]
pub struct GetTelemetryEvaluationStatusForOrganizationOutput {
    pub status: Option<String>,
    pub failure_reason: Option<String>,
    pub home_region: Option<String>,
    pub region_statuses: Option<Vec<RegionStatus>>,
}

// The operation's MODELED errors.
#[smithy_macros::operation_error(
    "com.amazonaws.observabilityadmin#GetTelemetryEvaluationStatusForOrganization"
)]
#[derive(Debug)]
pub enum GetTelemetryEvaluationStatusForOrganizationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    TooManyRequestsException(TooManyRequestsException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type GetTelemetryEvaluationStatusForOrganizationError =
    crate::scaffolded::Scaffolded<GetTelemetryEvaluationStatusForOrganizationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<GetTelemetryEvaluationStatusForOrganizationErrorModeled>
    for GetTelemetryEvaluationStatusForOrganizationError
{
    fn from(modeled: GetTelemetryEvaluationStatusForOrganizationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetTelemetryEvaluationStatusForOrganization` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetTelemetryEvaluationStatusForOrganization<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetTelemetryEvaluationStatusForOrganization<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)`.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetTelemetryEvaluationStatusForOrganization<Eng>
{
    type Input = GetTelemetryEvaluationStatusForOrganizationInput;
    type Output = GetTelemetryEvaluationStatusForOrganizationOutput;
    type Error = GetTelemetryEvaluationStatusForOrganizationError;

    const NAME: &'static str = "GetTelemetryEvaluationStatusForOrganization";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        GET_TELEMETRY_EVALUATION_STATUS_FOR_ORGANIZATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        GET_TELEMETRY_EVALUATION_STATUS_FOR_ORGANIZATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = &input;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetTelemetryEvaluationStatusForOrganizationErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("the telemetry evaluation store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;

        let region = crate::state::request_region(ctx.parts());
        let now = crate::state::now_epoch_millis();
        let scope = crate::state::EvaluationScope::Organization;

        let store = state.lock();
        let Some(record) = store.telemetry_evaluation(scope, &region) else {
            return Ok(GetTelemetryEvaluationStatusForOrganizationOutput {
                status: Some(crate::state::STATUS_NOT_STARTED.to_string()),
                ..Default::default()
            });
        };

        Ok(GetTelemetryEvaluationStatusForOrganizationOutput {
            status: Some(record.status_at(now).to_string()),
            failure_reason: None,
            home_region: record
                .is_multi_region()
                .then(|| record.home_region.clone()),
            region_statuses: store.evaluation_region_statuses(scope, record, now),
        })
    }
}
