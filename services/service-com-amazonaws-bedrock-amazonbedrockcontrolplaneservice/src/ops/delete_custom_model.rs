// SPDX-License-Identifier: CC0-1.0

//! @op DeleteCustomModel
//!
//! Removes ONE stored custom model from [`crate::state`] — the model
//! `ops/create_custom_model.rs` minted, or the one a completed customization or copy job
//! brought into being — together with its name index entry and the idempotency token that
//! minted it
//! ([`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::remove_custom_model`]),
//! so a later create of that name starts fresh and a later read of the model reports it
//! gone.
//!
//! The rules, each read off this operation's own members
//! (`smithy_query(query="operation",operation="DeleteCustomModel")`):
//!
//! * the caller has to be ALLOWED to perform the action: IAM decides that ahead of this
//!   service resolving the identifier, so a principal whose identity-based policies do not
//!   grant `bedrock:DeleteCustomModel` on the model the request addressed — or that an
//!   explicit `Deny` covers — is answered 403 `AccessDeniedException` by the layer every
//!   operation is served behind ([`crate::service::authorization`]) rather than told
//!   whether that model exists, and nothing is removed;
//! * `modelIdentifier` is the "Name of the model to delete", and the member's pattern
//!   admits that name or the model's ARN
//!   ([`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::custom_model`]),
//!   so both spellings delete the same model. An identifier that resolves to no custom
//!   model of this account and Region — including one a concurrent delete has already
//!   removed — is refused as the spelling it IS, the one reading of that failure every
//!   operation addressing this resource shares
//!   ([`crate::state::unresolved_model`]): 404 `ResourceNotFoundException` for an
//!   ARN, which is what that error documents ("The specified resource Amazon Resource Name
//!   (ARN) was not found"), carrying this resource's not-found sentence
//!   ([`crate::state::CUSTOM_MODEL_NOT_FOUND_MESSAGE`]); while a bare NAME that resolves to
//!   no model is no ARN to report missing but an identifier this service could not resolve,
//!   which is 400 `ValidationException` carrying the one sentence Amazon Bedrock states for
//!   that class ([`crate::state::INVALID_MODEL_IDENTIFIER_MESSAGE`]). Neither sentence
//!   quotes any part of the request. So of two deletes racing for one model exactly one
//!   deletes it and the other finds nothing left;
//! * a model a custom model deployment is still serving is NOT removed: that deployment
//!   would be left serving a model that no longer exists, which is the conflicting change
//!   `ConflictException` (400) names
//!   ([`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::custom_model_deployment_serving`]);
//!   the refusal names the model and the deployment holding the association, both read out
//!   of the store, in a sentence CHOSEN here rather than recorded — see the branch itself.
//!
//! The delete is a MUTATION — "Deletes a custom model that you created earlier" — and the
//! only other write it makes is the transition the CLOCK already made due for the record it
//! resolves, which the resolution itself materialises: a model whose customization job has
//! already completed is a model this call can delete, exactly as it is a model a read
//! reports.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteCustomModel`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Deletes a custom model that you created earlier. For more information, see Custom models
//! in the Amazon Bedrock User Guide.
//!
//! ## Input
//!
//! * `modelIdentifier` (ModelIdentifier, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:(([0-9]{12}:custom-model/((imported)|([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}))((
//!   …) **required** — Name of the model to delete.
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
// `DeleteCustomModelError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteCustomModelError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#DeleteCustomModel")]
#[derive(Debug)]
pub enum DeleteCustomModelErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_custom_model::DeleteCustomModelError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteCustomModelError = crate::scaffolded::Scaffolded<DeleteCustomModelErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteCustomModelErrorModeled> for DeleteCustomModelError {
    fn from(modeled: DeleteCustomModelErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteCustomModel` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteCustomModel<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteCustomModel<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteCustomModel<Eng>
{
    type Input = DeleteCustomModelInput;
    type Output = DeleteCustomModelOutput;
    type Error = DeleteCustomModelError;

    const NAME: &'static str = "DeleteCustomModel";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_CUSTOM_MODEL_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_CUSTOM_MODEL_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DeleteCustomModelErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("custom model state is not available on this engine".to_string()),
                },
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        // Either `ModelIdentifier` spelling addresses the same model, and the resolution
        // materialises the transition the clock already owes the record it finds — so this
        // call deletes exactly the model a read at the same instant would report.
        let Some(arn) = store
            .custom_model(&input.model_identifier, &region, now)
            .map(|record| record.arn.clone())
        else {
            // An identifier that resolves to nothing is refused as the spelling it IS —
            // the one reading of that failure every model-identifying member shares
            // ([`crate::state::unresolved_model`]), answered here with the errors this
            // operation's own model declares for the two classes.
            let refusal = crate::state::unresolved_model(
                &input.model_identifier,
                crate::state::CUSTOM_MODEL_RESOURCES,
            );
            let message = Some(refusal.message(&input.model_identifier));
            return Err(if refusal.missing_resource() {
                DeleteCustomModelErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException { message },
                )
            } else {
                DeleteCustomModelErrorModeled::ValidationException(ValidationException { message })
            }
            .into());
        };

        // A deployment still serving this model would be left serving a model that is
        // gone, so the association is the conflicting change and the model stays.
        //
        // The SENTENCE is this emulator's own: no recording of this service fixes a wording
        // for this conflict, and the error itself documents only the class ("Error occurred
        // because of a conflict while performing an operation"). So it carries no fixed text
        // taken from anywhere else — what it states is the two facts the store holds, the
        // model this call would have removed and the deployment that still serves it, so the
        // caller knows which resource to delete first.
        if let Some(associated) = store.custom_model_deployment_serving(&arn) {
            return Err(DeleteCustomModelErrorModeled::ConflictException(
                ConflictException {
                    message: Some(format!(
                        "Custom model {arn} cannot be deleted while it is served by the \
                         custom model deployment {associated}."
                    )),
                },
            )
            .into());
        }

        store.remove_custom_model(&arn);
        Ok(DeleteCustomModelOutput {})
    }
}
