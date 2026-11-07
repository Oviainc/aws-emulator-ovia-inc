// SPDX-License-Identifier: CC0-1.0

//! @op GetCloudWatchAlarmTemplate
//! Retrieves the specified cloudwatch alarm template out of this service's
//! state.
//!
//! This is the ONLY file to touch for this operation. Shared types live in
//! `super::types`; the store lives in `crate::state::alarm_templates`.
//!
//! ---
//!
//! # Modeled contract for `GetCloudWatchAlarmTemplate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medialive-medialive/smithy-model/com-amazonaws-medialive-medialive.json")
//! ```
//!
//! Retrieves the specified cloudwatch alarm template.
//!
//! ## Input
//!
//! * `Identifier` (__string) **required** — A cloudwatch alarm template's identifier. Can be
//!   either be its id or current name.
//!
//! ## Output
//!
//! * `Arn` (__stringPatternArnMedialiveCloudwatchAlarmTemplate) — A cloudwatch alarm
//!   template's ARN (Amazon Resource Name)
//! * `ComparisonOperator` (CloudWatchAlarmTemplateComparisonOperator, one of:
//!   GreaterThanOrEqualToThreshold | GreaterThanThreshold | LessThanOrEqualToThreshold |
//!   LessThanThreshold) — The comparison operator used to compare the specified statistic and
//!   the threshold.
//! * `CreatedAt` (__timestampIso8601) — Placeholder documentation for __timestampIso8601
//! * `DatapointsToAlarm` (__integerMin1) — The number of datapoints within the evaluation
//!   period that must be breaching to trigger the alarm.
//! * `Description` (__stringMin0Max1024) — A resource's optional description.
//! * `EvaluationPeriods` (__integerMin1) — The number of periods over which data is compared
//!   to the specified threshold.
//! * `GroupId` (__stringMin7Max11PatternAws097) — A cloudwatch alarm template group's id. AWS
//!   provided template groups have ids that start with `aws-`
//! * `Id` (__stringMin7Max11PatternAws097) — A cloudwatch alarm template's id. AWS provided
//!   templates have ids that start with `aws-`
//! * `MetricName` (__stringMax64) — The name of the metric associated with the alarm. Must be
//!   compatible with targetResourceType.
//! * `ModifiedAt` (__timestampIso8601) — Placeholder documentation for __timestampIso8601
//! * `Name` (__stringMin1Max255PatternS) — A resource's name. Names must be unique within the
//!   scope of a resource type in a specific region.
//! * `Period` (__integerMin10Max86400) — The period, in seconds, over which the specified
//!   statistic is applied.
//! * `Statistic` (CloudWatchAlarmTemplateStatistic, one of: Average | Maximum | Minimum |
//!   SampleCount | Sum) — The statistic to apply to the alarm's metric data.
//! * `Tags` (TagMap) — Represents the tags associated with a resource.
//! * `TargetResourceType` (CloudWatchAlarmTemplateTargetResourceType, one of:
//!   CLOUDFRONT_DISTRIBUTION | ELEMENTAL_INFERENCE_FEED | MEDIACONNECT_FLOW |
//!   MEDIALIVE_CHANNEL | MEDIALIVE_INPUT_DEVICE | MEDIALIVE_MULTIPLEX | MEDIAPACKAGE_CHANNEL
//!   | MEDIAPACKAGE_ORIGIN_ENDPOINT | MEDIATAILOR_PLAYBACK_CONFIGURATION | S3_BUCKET) — The
//!   resource type this template should dynamically generate cloudwatch metric alarms for.
//! * `Threshold` (__double) — The threshold value to compare with the specified statistic.
//! * `TreatMissingData` (CloudWatchAlarmTemplateTreatMissingData, one of: breaching | ignore
//!   | missing | notBreaching) — Specifies how missing data points are treated when
//!   evaluating the alarm's condition.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — Placeholder documentation for BadRequestException
//! * `ForbiddenException` (HTTP 403) — Placeholder documentation for ForbiddenException
//! * `InternalServerErrorException` (HTTP 500) — Placeholder documentation for
//!   InternalServerErrorException
//! * `NotFoundException` (HTTP 404) — Placeholder documentation for NotFoundException
//! * `TooManyRequestsException` (HTTP 429) — Placeholder documentation for
//!   TooManyRequestsException

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetCloudWatchAlarmTemplateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medialive-medialive.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetCloudWatchAlarmTemplateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medialive#GetCloudWatchAlarmTemplate")]
#[derive(Debug)]
pub enum GetCloudWatchAlarmTemplateErrorModeled {
    BadRequestException(BadRequestException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_cloud_watch_alarm_template::GetCloudWatchAlarmTemplateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetCloudWatchAlarmTemplateError = crate::scaffolded::Scaffolded<GetCloudWatchAlarmTemplateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetCloudWatchAlarmTemplateErrorModeled> for GetCloudWatchAlarmTemplateError {
    fn from(modeled: GetCloudWatchAlarmTemplateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetCloudWatchAlarmTemplate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetCloudWatchAlarmTemplate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetCloudWatchAlarmTemplate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetCloudWatchAlarmTemplate<Eng>
{
    type Input = GetCloudWatchAlarmTemplateInput;
    type Output = GetCloudWatchAlarmTemplateOutput;
    type Error = GetCloudWatchAlarmTemplateError;

    const NAME: &'static str = "GetCloudWatchAlarmTemplate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_CLOUD_WATCH_ALARM_TEMPLATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_CLOUD_WATCH_ALARM_TEMPLATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine, ctx.parts()).ok_or_else(unavailable)?;
        let region = crate::state::request_region(ctx.parts());

        // "Retrieves the specified cloudwatch alarm template" — a pure read:
        // nothing here writes, so what a later caller sees is unchanged.
        let state = state.lock();
        let template = state
            .alarm_template(&region, &input.identifier)
            .ok_or_else(not_found)?;

        Ok(crate::state::alarm_template_output!(
            GetCloudWatchAlarmTemplateOutput,
            template
        ))
    }
}

/// No template in this region has that id or name. The operation's own modelled
/// 404, carrying no `message` member — a resource of the cloudwatch
/// alarm-template hierarchy that an identifier does not resolve to is reported
/// with the status and the error code alone (see
/// [`crate::state::alarm_template_groups::CLOUD_WATCH_ALARM_TEMPLATE_NOT_FOUND_MESSAGE`]).
fn not_found() -> GetCloudWatchAlarmTemplateError {
    GetCloudWatchAlarmTemplateErrorModeled::NotFoundException(NotFoundException {
        message: crate::state::alarm_template_groups::CLOUD_WATCH_ALARM_TEMPLATE_NOT_FOUND_MESSAGE,
    })
    .into()
}

/// The engine this operation was built from is not the shared in-memory engine,
/// so there is no store to read the template from. A modelled server error, never
/// a fresh engine (which would answer from an empty private store).
fn unavailable() -> GetCloudWatchAlarmTemplateError {
    GetCloudWatchAlarmTemplateErrorModeled::InternalServerErrorException(
        InternalServerErrorException {
            message: Some(crate::state::NO_STORE.to_string()),
        },
    )
    .into()
}
