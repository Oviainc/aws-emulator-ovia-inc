// SPDX-License-Identifier: CC0-1.0

//! @op GetProvisionedModelThroughput
//!
//! Reports the Provisioned Throughput a `ProvisionedModelId` names out of
//! [`crate::state`] — by its ARN or by its name, the two spellings the member admits
//! ([`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::provisioned_model_arn_of`])
//! — or refuses an identifier that names no stored Provisioned Throughput with 404
//! `ResourceNotFoundException`.
//!
//! `@readonly`, and the one write it makes is the status transition the CLOCK already made
//! due ([`crate::state::ProvisionedModelThroughputRecord::settle`]): a Provisioned
//! Throughput whose `status` was owed
//! [`crate::state::PROVISIONED_MODEL_STATUS_IN_SERVICE`] before this call is reported, and
//! left, serving — which it would have been at that same instant whether or not anybody
//! read it. Nothing else about the record changes.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetProvisionedModelThroughput`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Returns details for a Provisioned Throughput. For more information, see Provisioned
//! Throughput in the Amazon Bedrock User Guide.
//!
//! ## Input
//!
//! * `provisionedModelId` (ProvisionedModelId, pattern
//!   ^((([0-9a-zA-Z][_-]?)+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:provisioned-model/[a-z0-9]{12}))$)
//!   **required** — The Amazon Resource Name (ARN) or name of the Provisioned Throughput.
//!
//! ## Output
//!
//! * `commitmentDuration` (CommitmentDuration, one of: OneMonth | SixMonths) — Commitment
//!   duration of the Provisioned Throughput.
//! * `commitmentExpirationTime` (Timestamp) — The timestamp for when the commitment term for
//!   the Provisioned Throughput expires.
//! * `creationTime` (Timestamp) **required** — The timestamp of the creation time for this
//!   Provisioned Throughput.
//! * `desiredModelArn` (ModelArn, len 20..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:(([0-9]{12}:custom-model/((imported)|([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}))((
//!   …) **required** — The Amazon Resource Name (ARN) of the model requested to be associated
//!   to this Provisioned Throughput. This value differs from the modelArn if updating hasn't
//!   completed.
//! * `desiredModelUnits` (PositiveInteger, range 1..) **required** — The number of model
//!   units that was requested for this Provisioned Throughput.
//! * `failureMessage` (ErrorMessage, len ..2048) — A failure message for any issues that
//!   occurred during creation, updating, or deletion of the Provisioned Throughput.
//! * `foundationModelArn` (FoundationModelArn, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}::foundation-model/[a-z0-9-]{1,63}[.]{1}([a-z0-9-]{1,63}[.]){0,2}[a-z0-9-]{1,63
//!   …) **required** — The Amazon Resource Name (ARN) of the base model for which the
//!   Provisioned Throughput was created, or of the base model that the custom model for which
//!   the Provisioned Throughput was created was customized.
//! * `lastModifiedTime` (Timestamp) **required** — The timestamp of the last time that this
//!   Provisioned Throughput was modified.
//! * `modelArn` (ModelArn, len 20..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:(([0-9]{12}:custom-model/((imported)|([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}))((
//!   …) **required** — The Amazon Resource Name (ARN) of the model associated with this
//!   Provisioned Throughput.
//! * `modelUnits` (PositiveInteger, range 1..) **required** — The number of model units
//!   allocated to this Provisioned Throughput.
//! * `provisionedModelArn` (ProvisionedModelArn, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:provisioned-model/[a-z0-9]{12}$)
//!   **required** — The Amazon Resource Name (ARN) of the Provisioned Throughput.
//! * `provisionedModelName` (ProvisionedModelName, len 1..63, pattern ^([0-9a-zA-Z][_-]?)+$)
//!   **required** — The name of the Provisioned Throughput.
//! * `status` (ProvisionedModelStatus, one of: Creating | Failed | InService | Updating)
//!   **required** — The status of the Provisioned Throughput.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetProvisionedModelThroughputError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetProvisionedModelThroughputError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#GetProvisionedModelThroughput")]
#[derive(Debug)]
pub enum GetProvisionedModelThroughputErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_provisioned_model_throughput::GetProvisionedModelThroughputError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetProvisionedModelThroughputError = crate::scaffolded::Scaffolded<GetProvisionedModelThroughputErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetProvisionedModelThroughputErrorModeled> for GetProvisionedModelThroughputError {
    fn from(modeled: GetProvisionedModelThroughputErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetProvisionedModelThroughput` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetProvisionedModelThroughput<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetProvisionedModelThroughput<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetProvisionedModelThroughput<Eng>
{
    type Input = GetProvisionedModelThroughputInput;
    type Output = GetProvisionedModelThroughputOutput;
    type Error = GetProvisionedModelThroughputError;

    const NAME: &'static str = "GetProvisionedModelThroughput";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PROVISIONED_MODEL_THROUGHPUT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PROVISIONED_MODEL_THROUGHPUT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                GetProvisionedModelThroughputErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "provisioned throughput state is not available on this engine"
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        };
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        let Some(record) =
            store.provisioned_model_throughput(&input.provisioned_model_id, &region, now)
        else {
            return Err(
                GetProvisionedModelThroughputErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(format!(
                            "no Provisioned Throughput {} exists in this account",
                            input.provisioned_model_id
                        )),
                    },
                )
                .into(),
            );
        };

        Ok(GetProvisionedModelThroughputOutput {
            model_units: record.model_units,
            desired_model_units: record.desired_model_units,
            provisioned_model_name: record.name.clone(),
            provisioned_model_arn: record.arn.clone(),
            model_arn: record.model_arn.clone(),
            desired_model_arn: record.desired_model_arn.clone(),
            foundation_model_arn: record.foundation_model_arn.clone(),
            status: record.status.to_string(),
            creation_time: crate::state::date_time(record.created_at),
            last_modified_time: crate::state::date_time(record.last_modified_at),
            // `failureMessage` is "a failure message for any issues that occurred during
            // creation, updating, or deletion of the Provisioned Throughput". Nothing here
            // makes an accepted Provisioned Throughput fail, so there is none to report.
            failure_message: None,
            commitment_duration: record.commitment_duration.clone(),
            commitment_expiration_time: record.commitment_expiration_time.map(crate::state::date_time),
        })
    }
}
