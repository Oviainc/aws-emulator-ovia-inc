// SPDX-License-Identifier: CC0-1.0

//! @op CreateCloudWatchAlarmTemplate
//! Creates a cloudwatch alarm template inside the group its `GroupIdentifier`
//! names, and answers the stored record.
//!
//! This is the ONLY file to touch for this operation. Shared types live in
//! `super::types`; the store and its invariants live in
//! `crate::state::alarm_templates`.
//!
//! ---
//!
//! # Modeled contract for `CreateCloudWatchAlarmTemplate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medialive-medialive/smithy-model/com-amazonaws-medialive-medialive.json")
//! ```
//!
//! Creates a cloudwatch alarm template to dynamically generate cloudwatch metric alarms on
//! targeted resource types.
//!
//! ## Input
//!
//! * `ComparisonOperator` (CloudWatchAlarmTemplateComparisonOperator, one of:
//!   GreaterThanOrEqualToThreshold | GreaterThanThreshold | LessThanOrEqualToThreshold |
//!   LessThanThreshold) **required** — The comparison operator used to compare the specified
//!   statistic and the threshold.
//! * `DatapointsToAlarm` (__integerMin1) — The number of datapoints within the evaluation
//!   period that must be breaching to trigger the alarm.
//! * `Description` (__stringMin0Max1024) — A resource's optional description.
//! * `EvaluationPeriods` (__integerMin1) **required** — The number of periods over which data
//!   is compared to the specified threshold.
//! * `GroupIdentifier` (__stringPatternS) **required** — A cloudwatch alarm template group's
//!   identifier. Can be either be its id or current name.
//! * `MetricName` (__stringMax64) **required** — The name of the metric associated with the
//!   alarm. Must be compatible with targetResourceType.
//! * `Name` (__stringMin1Max255PatternS) **required** — A resource's name. Names must be
//!   unique within the scope of a resource type in a specific region.
//! * `Period` (__integerMin10Max86400) **required** — The period, in seconds, over which the
//!   specified statistic is applied.
//! * `RequestId` (__stringMin1Max256PatternS, @idempotencyToken) — An ID that you assign to a
//!   create request. This ID ensures idempotency when creating resources.
//! * `Statistic` (CloudWatchAlarmTemplateStatistic, one of: Average | Maximum | Minimum |
//!   SampleCount | Sum) **required** — The statistic to apply to the alarm's metric data.
//! * `Tags` (TagMap) — Represents the tags associated with a resource.
//! * `TargetResourceType` (CloudWatchAlarmTemplateTargetResourceType, one of:
//!   CLOUDFRONT_DISTRIBUTION | ELEMENTAL_INFERENCE_FEED | MEDIACONNECT_FLOW |
//!   MEDIALIVE_CHANNEL | MEDIALIVE_INPUT_DEVICE | MEDIALIVE_MULTIPLEX | MEDIAPACKAGE_CHANNEL
//!   | MEDIAPACKAGE_ORIGIN_ENDPOINT | MEDIATAILOR_PLAYBACK_CONFIGURATION | S3_BUCKET)
//!   **required** — The resource type this template should dynamically generate cloudwatch
//!   metric alarms for.
//! * `Threshold` (__double) **required** — The threshold value to compare with the specified
//!   statistic.
//! * `TreatMissingData` (CloudWatchAlarmTemplateTreatMissingData, one of: breaching | ignore
//!   | missing | notBreaching) **required** — Specifies how missing data points are treated
//!   when evaluating the alarm's condition.
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
//! * `ConflictException` (HTTP 409) — Placeholder documentation for ConflictException
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
// `CreateCloudWatchAlarmTemplateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medialive-medialive.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateCloudWatchAlarmTemplateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medialive#CreateCloudWatchAlarmTemplate")]
#[derive(Debug)]
pub enum CreateCloudWatchAlarmTemplateErrorModeled {
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_cloud_watch_alarm_template::CreateCloudWatchAlarmTemplateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateCloudWatchAlarmTemplateError = crate::scaffolded::Scaffolded<CreateCloudWatchAlarmTemplateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateCloudWatchAlarmTemplateErrorModeled> for CreateCloudWatchAlarmTemplateError {
    fn from(modeled: CreateCloudWatchAlarmTemplateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateCloudWatchAlarmTemplate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateCloudWatchAlarmTemplate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateCloudWatchAlarmTemplate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateCloudWatchAlarmTemplate<Eng>
{
    type Input = CreateCloudWatchAlarmTemplateInput;
    type Output = CreateCloudWatchAlarmTemplateOutput;
    type Error = CreateCloudWatchAlarmTemplateError;

    const NAME: &'static str = "CreateCloudWatchAlarmTemplate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CLOUD_WATCH_ALARM_TEMPLATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CLOUD_WATCH_ALARM_TEMPLATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 201u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine, ctx.parts()).ok_or_else(unavailable)?;

        // A tag key in the reserved namespace names a system tag, which is not
        // the caller's to set, so a request carrying one is refused before
        // anything is stored rather than creating a resource that claims it.
        if let Some(key) = crate::state::reserved_tag_key_in(input.tags.as_ref()) {
            return Err(CreateCloudWatchAlarmTemplateErrorModeled::BadRequestException(
                BadRequestException {
                    message: Some(crate::state::reserved_tag_key_message(key)),
                },
            )
            .into());
        }

        // The settings this request asks the template to carry, against the ONE
        // rule set the alarm template slice states for them — the length bounds,
        // the closed enum catalogs, the numeric ranges and the finite threshold —
        // so this create and the update beside it cannot read one constraint on
        // one member of this resource two ways. A create names every setting, so
        // every rule applies here.
        crate::state::check_alarm_template_settings(&crate::state::AlarmTemplateSettings {
            name: Some(input.name.as_str()),
            description: input.description.as_deref(),
            request_id: input.request_id.as_deref(),
            metric_name: Some(input.metric_name.as_str()),
            comparison_operator: Some(input.comparison_operator.as_str()),
            statistic: Some(input.statistic.as_str()),
            target_resource_type: Some(input.target_resource_type.as_str()),
            treat_missing_data: Some(input.treat_missing_data.as_str()),
            period: Some(input.period),
            evaluation_periods: Some(input.evaluation_periods),
            datapoints_to_alarm: input.datapoints_to_alarm,
            threshold: Some(input.threshold),
        })
        .map_err(bad_request)?;

        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();

        let mut state = state.lock();
        // Clock-owed work first, as every operation of this service does: the
        // transitions that were already due at this instant were owed whether
        // or not this create ran, so the token lookup and the mint below see
        // the store a reader would.
        state.settle(now);

        // A retried create (same `@idempotencyToken` `RequestId`) must return
        // the template the first one created, not a second template: "This ID
        // ensures idempotency when creating resources".
        if let Some(existing) = input
            .request_id
            .as_deref()
            .and_then(|token| state.alarm_template_for_token(&region, token))
        {
            return Ok(crate::state::alarm_template_output!(
                CreateCloudWatchAlarmTemplateOutput,
                existing
            ));
        }

        // The group resolution, the name-uniqueness rule, the store's cap and
        // the mint are one step in the store, so nothing can pass one check and
        // write past another; what is left here is turning each refusal into
        // this operation's own modelled error.
        let id = state
            .insert_alarm_template(
                &region,
                &input.group_identifier,
                crate::state::NewCloudWatchAlarmTemplate {
                    name: input.name,
                    description: input.description,
                    comparison_operator: input.comparison_operator,
                    datapoints_to_alarm: input.datapoints_to_alarm,
                    evaluation_periods: input.evaluation_periods,
                    metric_name: input.metric_name,
                    period: input.period,
                    statistic: input.statistic,
                    target_resource_type: input.target_resource_type,
                    threshold: input.threshold,
                    treat_missing_data: input.treat_missing_data,
                    tags: input.tags.unwrap_or_default(),
                },
                now,
            )
            .map_err(refused)?;

        if let Some(token) = input.request_id.as_deref() {
            // Bounded, evicting write, keyed by the region the token was
            // presented in: a caller-chosen token can neither accumulate past
            // the cap nor outlive the template it created, and two regions using
            // one token do not overwrite each other.
            state.record_alarm_template_token(&region, token, &id);
        }

        // The response is read back out of the stored record, never built from
        // `input`.
        let template = state.alarm_template(&region, &id).ok_or_else(unavailable)?;
        Ok(crate::state::alarm_template_output!(
            CreateCloudWatchAlarmTemplateOutput,
            template
        ))
    }
}

/// Turn a store refusal into this operation's own modelled error: a group the
/// region does not hold is its 404, a name the region already uses its 409, and a
/// full store its 429.
///
/// The 404 carries no `message` member: the group is a resource of the
/// cloudwatch alarm-template hierarchy, which reports a resource an identifier
/// does not resolve to with the status and the error code alone (see
/// [`crate::state::alarm_template_groups::CLOUD_WATCH_ALARM_TEMPLATE_NOT_FOUND_MESSAGE`]).
fn refused(
    refusal: crate::state::AlarmTemplateRefusal,
) -> CreateCloudWatchAlarmTemplateError {
    match refusal {
        crate::state::AlarmTemplateRefusal::GroupNotFound { .. } => {
            CreateCloudWatchAlarmTemplateErrorModeled::NotFoundException(NotFoundException {
                message:
                    crate::state::alarm_template_groups::CLOUD_WATCH_ALARM_TEMPLATE_NOT_FOUND_MESSAGE,
            })
            .into()
        }
        crate::state::AlarmTemplateRefusal::NameTaken { .. } => {
            CreateCloudWatchAlarmTemplateErrorModeled::ConflictException(ConflictException {
                message: Some(crate::state::NAME_ALREADY_IN_USE_MESSAGE.to_string()),
            })
            .into()
        }
        crate::state::AlarmTemplateRefusal::StoreFull => {
            CreateCloudWatchAlarmTemplateErrorModeled::TooManyRequestsException(
                TooManyRequestsException {
                    message: Some(format!(
                        "This emulator stores at most {} cloudwatch alarm templates; delete one before creating another",
                        crate::state::MAX_ALARM_TEMPLATES
                    )),
                },
            )
            .into()
        }
    }
}

/// This operation's modelled 400, for a request the model's own constraints do
/// not already refuse.
fn bad_request(message: String) -> CreateCloudWatchAlarmTemplateError {
    CreateCloudWatchAlarmTemplateErrorModeled::BadRequestException(BadRequestException {
        message: Some(message),
    })
    .into()
}

/// The engine this operation was built from is not the shared in-memory engine,
/// so there is no store to create the template in. A modelled server error, never
/// a fresh engine (which would drop the write when this request returns).
fn unavailable() -> CreateCloudWatchAlarmTemplateError {
    CreateCloudWatchAlarmTemplateErrorModeled::InternalServerErrorException(
        InternalServerErrorException {
            message: Some(crate::state::NO_STORE.to_string()),
        },
    )
    .into()
}
