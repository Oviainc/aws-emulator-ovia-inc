// SPDX-License-Identifier: CC0-1.0

//! @op GetApplicationVersion
//!
//! Reports one recorded version of a persisted application: the definition text
//! Create/Update persisted for it, its description, its status and when it was recorded.
//! An unknown application id, or a version number that application never recorded, is a
//! 404 `ResourceNotFoundException`.
//!
//! This is a REPORTING operation (`@readonly` in the model): it writes nothing, and the
//! `Creating` -> `Available` transition the clock owes a new version is computed from the
//! stored settle instant (see [`crate::state::CREATION_SETTLES_AFTER`]).
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), reached through
//! [`crate::state::service_state`]. `ctx` carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `GetApplicationVersion`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-m2-awssupernovacontrolplaneservice/smithy-model/com-amazonaws-m2-awssupernovacontrolplaneservice.json")
//! ```
//!
//! Returns details about a specific version of a specific application.
//!
//! ## Input
//!
//! * `applicationId` (Identifier, pattern ^\S{1,80}$) **required** — The unique identifier of
//!   the application.
//! * `applicationVersion` (Version, range 1..) **required** — The specific version of the
//!   application.
//!
//! ## Output
//!
//! * `applicationVersion` (Version, range 1..) **required** — The specific version of the
//!   application.
//! * `creationTime` (Timestamp) **required** — The timestamp when the application version was
//!   created.
//! * `definitionContent` (StringFree65000, len 1..65000) **required** — The content of the
//!   application definition. This is a JSON object that contains the resource configuration
//!   and definitions that identify an application.
//! * `description` (EntityDescription, len 0..500) — The application description.
//! * `name` (EntityName, pattern ^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$) **required** — The name
//!   of the application version.
//! * `status` (ApplicationVersionLifecycle, one of: Available | Creating | Failed)
//!   **required** — The status of the application version.
//! * `statusReason` (String) — The reason for the reported status.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The account or role doesn't have the right
//!   permissions to make the request.
//! * `InternalServerException` (HTTP 500) — An unexpected error occurred during the
//!   processing of the request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource was not found.
//! * `ThrottlingException` (HTTP 429) — The number of requests made exceeds the limit.
//! * `ValidationException` (HTTP 400) — One or more parameters provided in the request is not
//!   valid.

#[allow(unused_imports)]
use super::types::*;

use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetApplicationVersionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-m2-awssupernovacontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetApplicationVersionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.m2#GetApplicationVersion")]
#[derive(Debug)]
pub enum GetApplicationVersionErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_application_version::GetApplicationVersionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetApplicationVersionError =
    crate::scaffolded::Scaffolded<GetApplicationVersionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetApplicationVersionErrorModeled> for GetApplicationVersionError {
    fn from(modeled: GetApplicationVersionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetApplicationVersion` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetApplicationVersion<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetApplicationVersion<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetApplicationVersion<Eng>
{
    type Input = GetApplicationVersionInput;
    type Output = GetApplicationVersionOutput;
    type Error = GetApplicationVersionError;

    const NAME: &'static str = "GetApplicationVersion";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_APPLICATION_VERSION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_APPLICATION_VERSION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetApplicationVersionErrorModeled::InternalServerException(InternalServerException {
                message: "the m2 application store is unavailable".to_string(),
                retry_after_seconds: 0,
            })
        })?;

        let now = SystemTime::now();
        let store = state.lock();
        let application = store
            .applications
            .get(&input.application_id)
            .ok_or_else(|| {
                GetApplicationVersionErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: format!("no application with the id {}", input.application_id),
                        resource_id: Some(input.application_id.clone()),
                        resource_type: Some("application".to_string()),
                    },
                )
            })?;

        let version = application
            .version(input.application_version)
            .ok_or_else(|| {
                GetApplicationVersionErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: format!(
                            "application {} has no version {}",
                            input.application_id, input.application_version
                        ),
                        resource_id: Some(input.application_id.clone()),
                        resource_type: Some("application-version".to_string()),
                    },
                )
            })?;

        Ok(GetApplicationVersionOutput {
            name: application.name.clone(),
            application_version: version.application_version,
            description: version.description.clone(),
            definition_content: version.definition_content.clone(),
            status: version.status(now).to_string(),
            creation_time: ::aws_smithy_types::DateTime::from(version.creation_time),
            status_reason: version.status_reason.clone(),
        })
    }
}
