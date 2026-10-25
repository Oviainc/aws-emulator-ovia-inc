// SPDX-License-Identifier: CC0-1.0

//! @op GetCustomModelDeployment
//!
//! Reports one custom model deployment out of [`crate::state`]: the deployment
//! `ops/create_custom_model_deployment.rs` minted, addressed by either
//! `CustomModelDeploymentIdentifier` spelling — its ARN or its name — and reported with
//! the status it holds at the instant of the call.
//!
//! A REPORTING operation (`@readonly`): the one write it makes is through
//! [`crate::state::CustomModelDeploymentRecord::settle`], which materialises the move to
//! [`crate::state::CUSTOM_MODEL_DEPLOYMENT_STATUS_ACTIVE`] the clock had already made due
//! at the same instant even if nobody had called.
//!
//! An identifier this account holds no deployment for is refused by its SPELLING
//! ([`names_deployment_arn`]): an ARN is a resource that is not there, so 404
//! `ResourceNotFoundException`, while a bare NAME has to be resolved to an ARN before
//! anything can be looked up, so one that resolves to none is 400 `ValidationException`
//! ([`crate::state::CUSTOM_MODEL_DEPLOYMENT_IDENTIFIER_INVALID`]).
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetCustomModelDeployment`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Retrieves information about a custom model deployment, including its status,
//! configuration, and metadata. Use this operation to monitor the deployment status and
//! retrieve details needed for inference requests. The following actions are related to the
//! GetCustomModelDeployment operation: CreateCustomModelDeployment
//! ListCustomModelDeployments DeleteCustomModelDeployment
//!
//! ## Input
//!
//! * `customModelDeploymentIdentifier` (CustomModelDeploymentIdentifier, len 1..93, pattern
//!   ^(arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:[a-z0-9-]{1,20}:[0-9]{12}:custom-model-deployment/[a-z0-9]{12})|^([0-9a-zA-Z
//!   …) **required** — The Amazon Resource Name (ARN) or name of the custom model deployment
//!   to retrieve information about.
//!
//! ## Output
//!
//! * `createdAt` (Timestamp) **required** — The date and time when the custom model
//!   deployment was created.
//! * `customModelDeploymentArn` (CustomModelDeploymentArn, len ..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:custom-model-deployment/[a-z0-9]{12}$)
//!   **required** — The Amazon Resource Name (ARN) of the custom model deployment.
//! * `description` (CustomModelDeploymentDescription, len 1..2048, pattern ^.*$) — The
//!   description of the custom model deployment.
//! * `failureMessage` (ErrorMessage, len ..2048) — If the deployment status is FAILED, this
//!   field contains a message describing the failure reason.
//! * `lastUpdatedAt` (Timestamp) — The date and time when the custom model deployment was
//!   last updated.
//! * `modelArn` (CustomModelArn, len 20..1011, pattern
//!   ^arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:[a-z0-9-]{1,20}:[0-9]{12}:custom-model/(imported|[a-z0-9-]{1,63}[.]{1}[a-z0-9
//!   …) **required** — The Amazon Resource Name (ARN) of the custom model associated with
//!   this deployment.
//! * `modelDeploymentName` (ModelDeploymentName, len 1..63, pattern
//!   ^([0-9a-zA-Z][_-]?){1,63}$) **required** — The name of the custom model deployment.
//! * `status` (CustomModelDeploymentStatus, one of: Active | Creating | Failed) **required**
//!   — The status of the custom model deployment. Possible values are: CREATING - The
//!   deployment is being set up and prepared for inference. ACTIVE - The deployment is ready
//!   and available for inference requests. FAILED - The deployment failed to be created or
//!   became unavailable.
//! * `updateDetails` (CustomModelDeploymentUpdateDetails) — Details about any pending or
//!   completed updates to the custom model deployment, including the new model ARN and update
//!   status.
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
// `GetCustomModelDeploymentError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetCustomModelDeploymentError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#GetCustomModelDeployment")]
#[derive(Debug)]
pub enum GetCustomModelDeploymentErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_custom_model_deployment::GetCustomModelDeploymentError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetCustomModelDeploymentError = crate::scaffolded::Scaffolded<GetCustomModelDeploymentErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetCustomModelDeploymentErrorModeled> for GetCustomModelDeploymentError {
    fn from(modeled: GetCustomModelDeploymentErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetCustomModelDeployment` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetCustomModelDeployment<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetCustomModelDeployment<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// Whether a `CustomModelDeploymentIdentifier` is the ARN spelling rather than the NAME
/// spelling.
///
/// The identifier is `<the deployment's ARN> | <its name>`
/// (`smithy_query(query="operation",operation="GetCustomModelDeployment")`), and the two
/// spellings are not read alike when neither names a stored deployment — which is the ONE
/// decision this read makes on the value's form, so it is read here rather than in
/// `crate::state`.
///
/// A TOTAL read of the value: the shared six-field ARN parse
/// (`service_common::arn::parse_of`, which answers `None` for anything that is not an ARN
/// of this service's namespace) and a comparison of the resource type it reports. Nothing
/// is indexed, sliced or unwrapped, so no value a caller can send reaches a panic here.
fn names_deployment_arn(identifier: &str) -> bool {
    ::service_common::arn::parse_of(identifier, ::service_common::bedrock::ARN_NAMESPACE)
        .is_some_and(|arn| {
            arn.resource_type() == crate::state::CUSTOM_MODEL_DEPLOYMENT_RESOURCE_TYPE
        })
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetCustomModelDeployment<Eng>
{
    type Input = GetCustomModelDeploymentInput;
    type Output = GetCustomModelDeploymentOutput;
    type Error = GetCustomModelDeploymentError;

    const NAME: &'static str = "GetCustomModelDeployment";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_CUSTOM_MODEL_DEPLOYMENT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_CUSTOM_MODEL_DEPLOYMENT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                GetCustomModelDeploymentErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "custom model deployment state is not available on this engine"
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
            store.custom_model_deployment(&input.custom_model_deployment_identifier, &region, now)
        else {
            // The two `CustomModelDeploymentIdentifier` spellings are not read alike when
            // neither names a stored deployment: an ARN is a resource that is not there,
            // while a NAME has to be resolved to an ARN first, and one that resolves to
            // none is not a usable identifier ([`names_deployment_arn`]).
            if !names_deployment_arn(&input.custom_model_deployment_identifier) {
                return Err(GetCustomModelDeploymentErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(
                            crate::state::CUSTOM_MODEL_DEPLOYMENT_IDENTIFIER_INVALID.to_string(),
                        ),
                    },
                )
                .into());
            }
            return Err(
                GetCustomModelDeploymentErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(format!(
                            "no custom model deployment {} exists in this account",
                            input.custom_model_deployment_identifier
                        )),
                    },
                )
                .into(),
            );
        };

        Ok(GetCustomModelDeploymentOutput {
            custom_model_deployment_arn: record.arn.clone(),
            model_deployment_name: record.name.clone(),
            model_arn: record.model_arn.clone(),
            created_at: crate::state::date_time(record.created_at),
            status: record.status.to_string(),
            description: record.description.clone(),
            // `updateDetails` reports "any pending or completed updates" to the
            // deployment. `UpdateCustomModelDeployment` is the only call that starts one
            // and is still a scaffold, so no stored deployment has an update to report.
            update_details: None,
            // `failureMessage` is reported "if the deployment status is FAILED". Nothing
            // here makes an accepted deployment fail, so there is none to report.
            failure_message: None,
            last_updated_at: record.last_updated_at.map(crate::state::date_time),
        })
    }
}

