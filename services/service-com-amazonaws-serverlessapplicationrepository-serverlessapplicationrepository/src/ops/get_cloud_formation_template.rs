// SPDX-License-Identifier: CC0-1.0

//! @op GetCloudFormationTemplate
//! Reports the prepared CloudFormation template `TemplateId` names, out of
//! [`crate::state`] — so it answers with exactly the record
//! `CreateCloudFormationTemplate` wrote.
//!
//! REPORTING ONLY: `Status` is derived from the stored instants and the clock
//! ([`crate::state::CloudFormationTemplate::status_at`]), so a read moves nothing and two
//! reads at one instant answer identically.
//!
//! A template is a sub-resource of its application, so one read through another
//! application's id is as absent as one that was never prepared (HTTP 404).
//!
//! This is the ONLY file to touch for this operation.
//!
//! The engine is held on `self` (constructor-injected); reach the store through
//! [`crate::state::service_state`]. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetCloudFormationTemplate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json")
//! ```
//!
//! Gets the specified AWS CloudFormation template.
//!
//! ## Input
//!
//! * `ApplicationId` (__string) **required** — The Amazon Resource Name (ARN) of the
//!   application.
//! * `TemplateId` (__string) **required** — The UUID returned by
//!   CreateCloudFormationTemplate. Pattern:
//!   [0-9a-fA-F]{8}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{12}
//!
//! ## Output
//!
//! * `ApplicationId` (__string) — The application Amazon Resource Name (ARN).
//! * `CreationTime` (__string) — The date and time this resource was created.
//! * `ExpirationTime` (__string) — The date and time this template expires. Templates expire
//!   1 hour after creation.
//! * `SemanticVersion` (__string) — The semantic version of the application:
//!   https://semver.org/
//! * `Status` (Status, one of: ACTIVE | EXPIRED | PREPARING) — Status of the template
//!   creation workflow. Possible values: PREPARING | ACTIVE | EXPIRED
//! * `TemplateId` (__string) — The UUID returned by CreateCloudFormationTemplate. Pattern:
//!   [0-9a-fA-F]{8}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{12}
//! * `TemplateUrl` (__string) — A link to the template that can be used to deploy the
//!   application using AWS CloudFormation.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — One of the parameters in the request is invalid.
//! * `ForbiddenException` (HTTP 403) — The client is not authenticated.
//! * `InternalServerErrorException` (HTTP 500) — The AWS Serverless Application Repository
//!   service encountered an internal error.
//! * `NotFoundException` (HTTP 404) — The resource (for example, an access policy statement)
//!   specified in the request doesn't exist.
//! * `TooManyRequestsException` (HTTP 429) — The client is sending more than the allowed
//!   number of requests per unit of time.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetCloudFormationTemplateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetCloudFormationTemplateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.serverlessapplicationrepository#GetCloudFormationTemplate")]
#[derive(Debug)]
pub enum GetCloudFormationTemplateErrorModeled {
    BadRequestException(BadRequestException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_cloud_formation_template::GetCloudFormationTemplateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetCloudFormationTemplateError = crate::scaffolded::Scaffolded<GetCloudFormationTemplateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetCloudFormationTemplateErrorModeled> for GetCloudFormationTemplateError {
    fn from(modeled: GetCloudFormationTemplateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetCloudFormationTemplate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetCloudFormationTemplate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetCloudFormationTemplate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetCloudFormationTemplate<Eng>
{
    type Input = GetCloudFormationTemplateInput;
    type Output = GetCloudFormationTemplateOutput;
    type Error = GetCloudFormationTemplateError;

    const NAME: &'static str = "GetCloudFormationTemplate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_CLOUD_FORMATION_TEMPLATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_CLOUD_FORMATION_TEMPLATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // AWS decides a signed `serverlessrepo` call against the CALLER's identity-based
        // policies BEFORE the operation reads anything, per the application the request
        // addresses; see [`crate::state::authorization`].
        crate::state::authorization::authorize(
            &*self.engine,
            ctx,
            Self::NAME,
            &input.application_id,
        )?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetCloudFormationTemplateErrorModeled::InternalServerErrorException(
                crate::state::internal_server_error("the service state is unavailable"),
            )
            .into());
        };

        let now = ::std::time::SystemTime::now();
        let store = state.lock();
        // The application is resolved first, so a read of a template under an application
        // that was never created — or has since been deleted — names the absent resource
        // the caller actually got wrong.
        if !store.applications.contains_key(&input.application_id) {
            return Err(not_found(
                crate::state::ErrorCode::ApplicationNotFound,
                crate::state::application_not_found_message(&input.application_id),
            ));
        }
        let Some(template) = store.template_of(&input.application_id, &input.template_id) else {
            return Err(not_found(
                crate::state::ErrorCode::TemplateNotFound,
                crate::state::template_not_found_message(&input.template_id),
            ));
        };

        let report = template.report(now);
        Ok(GetCloudFormationTemplateOutput {
            application_id: Some(report.application_id),
            creation_time: Some(report.creation_time),
            expiration_time: Some(report.expiration_time),
            semantic_version: Some(report.semantic_version),
            status: Some(report.status),
            template_id: Some(report.template_id),
            template_url: Some(report.template_url),
        })
    }
}

/// The 404 this operation answers for an application, or a prepared template of one, the
/// store does not hold. `code` names WHICH of the two was absent: the body's `errorCode`
/// carries the condition, never the status the status line already carries.
fn not_found(code: crate::state::ErrorCode, message: String) -> GetCloudFormationTemplateError {
    GetCloudFormationTemplateErrorModeled::NotFoundException(crate::state::not_found(code, message))
        .into()
}
