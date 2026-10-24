// SPDX-License-Identifier: CC0-1.0

//! @op DeleteProvisionedModelThroughput
//!
//! Deletes the Provisioned Throughput a `ProvisionedModelId` names — by its ARN or by its
//! name, the two spellings the member admits — and forgets its name index entry and the
//! idempotency token that minted it
//! ([`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::remove_provisioned_model_throughput`]),
//! so a later create of that name starts fresh.
//!
//! The refusal, on a rule rather than on one request: an identifier that names no stored
//! Provisioned Throughput — including one a concurrent delete has already removed — is 404
//! `ResourceNotFoundException`, so of two deletes racing for one Provisioned Throughput
//! exactly one deletes it and the other finds nothing left.
//!
//! `ConflictException` names a conflicting change to the same Provisioned Throughput; no
//! call implemented here leaves one in flight, so nothing mints it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteProvisionedModelThroughput`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Deletes a Provisioned Throughput. You can't delete a Provisioned Throughput before the
//! commitment term is over. For more information, see Provisioned Throughput in the Amazon
//! Bedrock User Guide.
//!
//! ## Input
//!
//! * `provisionedModelId` (ProvisionedModelId, pattern
//!   ^((([0-9a-zA-Z][_-]?)+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:provisioned-model/[a-z0-9]{12}))$)
//!   **required** — The Amazon Resource Name (ARN) or name of the Provisioned Throughput.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `ConflictException` (HTTP 400) — Error occurred because of a conflict while performing
//!   an operation.
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
// `DeleteProvisionedModelThroughputError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteProvisionedModelThroughputError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#DeleteProvisionedModelThroughput")]
#[derive(Debug)]
pub enum DeleteProvisionedModelThroughputErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_provisioned_model_throughput::DeleteProvisionedModelThroughputError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteProvisionedModelThroughputError = crate::scaffolded::Scaffolded<DeleteProvisionedModelThroughputErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteProvisionedModelThroughputErrorModeled> for DeleteProvisionedModelThroughputError {
    fn from(modeled: DeleteProvisionedModelThroughputErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteProvisionedModelThroughput` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteProvisionedModelThroughput<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteProvisionedModelThroughput<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteProvisionedModelThroughput<Eng>
{
    type Input = DeleteProvisionedModelThroughputInput;
    type Output = DeleteProvisionedModelThroughputOutput;
    type Error = DeleteProvisionedModelThroughputError;

    const NAME: &'static str = "DeleteProvisionedModelThroughput";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_PROVISIONED_MODEL_THROUGHPUT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_PROVISIONED_MODEL_THROUGHPUT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                DeleteProvisionedModelThroughputErrorModeled::InternalServerException(
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
        let mut store = state.lock();

        let Some(arn) = store.provisioned_model_arn_of(&input.provisioned_model_id, &region) else {
            return Err(
                DeleteProvisionedModelThroughputErrorModeled::ResourceNotFoundException(
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

        store.remove_provisioned_model_throughput(&arn);
        Ok(DeleteProvisionedModelThroughputOutput {})
    }
}
