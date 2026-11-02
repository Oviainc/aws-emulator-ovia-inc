// SPDX-License-Identifier: CC0-1.0

//! @op UpdateGlobalSettings
//!
//! Records the account-level global settings in [`crate::state`], where
//! `GetGlobalSettings` reads them back: whether the account is opted into organization
//! sharing and into discovery integration, and the account's Jira settings.
//!
//! A PARTIAL write, as the route it is bound to says (`PATCH /global-settings`,
//! `smithy_query(query="operation", operation="UpdateGlobalSettings")`): every input
//! member is optional, so a member the request names replaces what the account had
//! recorded for it and a member the request omits leaves that setting as it was. The
//! nested `JiraConfiguration` is merged the same way, member by member — a request that
//! names one Jira member does not erase the others.
//!
//! The values this operation records are what the read projects; what an account that has
//! recorded nothing reports instead lives with the record
//! ([`crate::state::GlobalSettingsRecord::to_global_settings`]), so both operations agree
//! about one set of settings.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back;
//! never construct an engine in a handler.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateGlobalSettings`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json")
//! ```
//!
//! Update whether the Amazon Web Services account is opted into organization sharing and
//! discovery integration features.
//!
//! ## Input
//!
//! * `DiscoveryIntegrationStatus` (DiscoveryIntegrationStatus, one of: DISABLED | ENABLED) —
//!   The status of discovery support settings.
//! * `JiraConfiguration` (AccountJiraConfigurationInput) — The status of Jira integration
//!   settings.
//! * `OrganizationSharingStatus` (OrganizationSharingStatus, one of: DISABLED | ENABLED) —
//!   The status of organization sharing settings.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — User does not have sufficient access to perform
//!   this action.
//! * `ConflictException` (HTTP 409) — The resource has already been processed, was deleted,
//!   or is too large.
//! * `InternalServerException` (HTTP 500) — There is a problem with the Well-Architected Tool
//!   API service.
//! * `ThrottlingException` (HTTP 429) — Request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The user input is not valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateGlobalSettingsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json");

#[derive(Debug, Clone, Default)]
pub struct UpdateGlobalSettingsOutput;

impl UpdateGlobalSettingsOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "PATCH",
        "/global-settings",
        Some(200u16),
    ));
}

pub static UPDATE_GLOBAL_SETTINGS_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &UpdateGlobalSettingsOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for UpdateGlobalSettingsOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateGlobalSettingsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.wellarchitected#UpdateGlobalSettings")]
#[derive(Debug)]
pub enum UpdateGlobalSettingsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_global_settings::UpdateGlobalSettingsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateGlobalSettingsError = crate::scaffolded::Scaffolded<UpdateGlobalSettingsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateGlobalSettingsErrorModeled> for UpdateGlobalSettingsError {
    fn from(modeled: UpdateGlobalSettingsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateGlobalSettings` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateGlobalSettings<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateGlobalSettings<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateGlobalSettings<Eng>
{
    type Input = UpdateGlobalSettingsInput;
    type Output = UpdateGlobalSettingsOutput;
    type Error = UpdateGlobalSettingsError;

    const NAME: &'static str = "UpdateGlobalSettings";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_GLOBAL_SETTINGS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = UPDATE_GLOBAL_SETTINGS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // `ctx` carries the request's Region, which is the store these settings belong
        // to: the settings are the account's IN one Region, as every resource of this
        // service is, and every request this emulator serves is attributed to its one
        // account.

        let state = crate::state::service_state(&*self.engine, ctx.parts()).ok_or_else(|| {
            UpdateGlobalSettingsErrorModeled::InternalServerException(InternalServerException {
                message: "the Well-Architected Tool global settings store is unavailable"
                    .to_string(),
            })
        })?;

        // Every modelled constraint (each status' enum value set, the project key's
        // pattern and length) is refused BEFORE this handler runs, so what arrives here
        // is only ever a value the account can be left holding.
        let mut store = state.lock();
        let settings = &mut store.global_settings;
        if let Some(status) = input.organization_sharing_status {
            settings.organization_sharing_status = Some(status);
        }
        if let Some(status) = input.discovery_integration_status {
            settings.discovery_integration_status = Some(status);
        }
        if let Some(jira) = input.jira_configuration {
            if let Some(status) = jira.integration_status {
                settings.jira_integration_status = Some(status);
            }
            if let Some(status) = jira.issue_management_status {
                settings.jira_issue_management_status = Some(status);
            }
            if let Some(issue_management_type) = jira.issue_management_type {
                settings.jira_issue_management_type = Some(issue_management_type);
            }
            if let Some(project_key) = jira.jira_project_key {
                settings.jira_project_key = Some(project_key);
            }
        }

        Ok(UpdateGlobalSettingsOutput)
    }
}
