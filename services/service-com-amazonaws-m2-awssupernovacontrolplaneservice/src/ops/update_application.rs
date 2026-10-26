// SPDX-License-Identifier: CC0-1.0

//! @op UpdateApplication
//!
//! Records a NEW version of a persisted application — the next version number, carrying
//! the supplied definition and description — and returns that number. The update is
//! guarded by `currentApplicationVersion`: a value that is not the application's latest
//! version is a 409 `ConflictException`, so two concurrent updates cannot both apply.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), reached through
//! [`crate::state::service_state`]. `ctx` carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `UpdateApplication`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-m2-awssupernovacontrolplaneservice/smithy-model/com-amazonaws-m2-awssupernovacontrolplaneservice.json")
//! ```
//!
//! Updates an application and creates a new version.
//!
//! ## Input
//!
//! * `applicationId` (Identifier, pattern ^\S{1,80}$) **required** — The unique identifier of
//!   the application you want to update.
//! * `currentApplicationVersion` (Version, range 1..) **required** — The current version of
//!   the application to update.
//! * `definition` (Definition) — The application definition for this application. You can
//!   specify either inline JSON or an S3 bucket location.
//! * `description` (EntityDescription, len 0..500) — The description of the application to
//!   update.
//!
//! ## Output
//!
//! * `applicationVersion` (Version, range 1..) **required** — The new version of the
//!   application.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The account or role doesn't have the right
//!   permissions to make the request.
//! * `ConflictException` (HTTP 409) — The parameters provided in the request conflict with
//!   existing resources.
//! * `InternalServerException` (HTTP 500) — An unexpected error occurred during the
//!   processing of the request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource was not found.
//! * `ThrottlingException` (HTTP 429) — The number of requests made exceeds the limit.
//! * `ValidationException` (HTTP 400) — One or more parameters provided in the request is not
//!   valid.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{ApplicationVersion, CREATION_SETTLES_AFTER};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateApplicationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-m2-awssupernovacontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateApplicationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.m2#UpdateApplication")]
#[derive(Debug)]
pub enum UpdateApplicationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_application::UpdateApplicationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateApplicationError = crate::scaffolded::Scaffolded<UpdateApplicationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateApplicationErrorModeled> for UpdateApplicationError {
    fn from(modeled: UpdateApplicationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateApplication` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateApplication<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateApplication<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateApplication<Eng>
{
    type Input = UpdateApplicationInput;
    type Output = UpdateApplicationOutput;
    type Error = UpdateApplicationError;

    const NAME: &'static str = "UpdateApplication";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_APPLICATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_APPLICATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UpdateApplicationErrorModeled::InternalServerException(InternalServerException {
                message: "the m2 application store is unavailable".to_string(),
                retry_after_seconds: 0,
            })
        })?;

        // An S3-located definition is refused with the modelled error rather than
        // persisted as invented content; see `crate::state::definition_content`.
        let definition = match input.definition.as_ref() {
            Some(definition) => Some(
                crate::state::definition_content(definition)
                    .ok_or_else(|| {
                        UpdateApplicationErrorModeled::ValidationException(ValidationException {
                            message: crate::state::S3_DEFINITION_UNSUPPORTED.to_string(),
                            reason: Some("featureNotAvailable".to_string()),
                            field_list: None,
                        })
                    })?
                    .to_string(),
            ),
            None => None,
        };

        let now = SystemTime::now();
        let mut store = state.lock();
        let application = store
            .applications
            .get_mut(&input.application_id)
            .ok_or_else(|| {
                UpdateApplicationErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: crate::state::application_not_found_message(
                            &input.application_id,
                        ),
                        resource_id: Some(input.application_id.clone()),
                        resource_type: Some("application".to_string()),
                    },
                )
            })?;

        // The update applies to the application's LATEST version only: a stale or
        // mismatched version means the caller is updating from a state that has moved.
        if input.current_application_version != application.latest_version {
            return Err(UpdateApplicationErrorModeled::ConflictException(ConflictException {
                message: format!(
                    "currentApplicationVersion {} is not the latest version {} of application {}",
                    input.current_application_version,
                    application.latest_version,
                    application.application_id
                ),
                resource_id: Some(application.application_id.clone()),
                resource_type: Some("application".to_string()),
            })
            .into());
        }

        // Each version keeps a full copy of the definition text and nothing short of a
        // delete drops one, so the history is capped and a further update is refused
        // rather than growing the process without limit. `UpdateApplication` models no
        // quota fault, so the refusal is its modelled `ValidationException`.
        if application.versions.len() >= crate::state::MAX_VERSIONS_PER_APPLICATION {
            return Err(UpdateApplicationErrorModeled::ValidationException(ValidationException {
                message: format!(
                    "application {} already has the {} versions this emulator keeps per \
                     application; no further version can be recorded",
                    application.application_id,
                    crate::state::MAX_VERSIONS_PER_APPLICATION
                ),
                reason: Some("other".to_string()),
                field_list: None,
            })
            .into());
        }

        // A member the caller did not supply keeps the value the previous version
        // recorded, so the new version is complete on its own.
        let previous = application.latest().cloned();
        let definition_content = match definition {
            Some(content) => content,
            None => previous
                .as_ref()
                .map(|version| version.definition_content.clone())
                .unwrap_or_default(),
        };
        let description = match input.description.clone() {
            Some(description) => Some(description),
            None => previous.and_then(|version| version.description),
        };

        let application_version = application
            .latest_version
            .checked_add(1)
            .ok_or_else(|| {
                UpdateApplicationErrorModeled::ValidationException(ValidationException {
                    message: format!(
                        "application {} is at the highest version number it can record",
                        application.application_id
                    ),
                    reason: Some("other".to_string()),
                    field_list: None,
                })
            })?;
        application.versions.push(ApplicationVersion {
            application_version,
            definition_content,
            description: description.clone(),
            creation_time: now,
            settles_at: now + CREATION_SETTLES_AFTER,
            status_reason: None,
        });
        application.latest_version = application_version;
        application.description = description;

        Ok(UpdateApplicationOutput {
            application_version,
        })
    }
}
