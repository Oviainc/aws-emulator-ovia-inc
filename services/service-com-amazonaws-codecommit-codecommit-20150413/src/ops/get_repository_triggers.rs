// SPDX-License-Identifier: CC0-1.0

//! @op GetRepositoryTriggers
//!
//! "Gets information about triggers configured for a repository": a pure read of the
//! configuration `PutRepositoryTriggers` stored — the triggers it was given and the
//! `configurationId` that write minted — so two reads of an untouched repository
//! report the same thing and a read changes nothing a later caller sees.
//!
//! A repository no caller has configured holds no configuration at all: it has no
//! trigger, which is the empty list, and no id, because no write has minted one.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetRepositoryTriggers`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codecommit-codecommit-20150413/smithy-model/com-amazonaws-codecommit-codecommit-20150413.json")
//! ```
//!
//! Gets information about triggers configured for a repository.
//!
//! ## Input
//!
//! * `repositoryName` (RepositoryName, len 1..100, pattern ^[\w\.-]+$) **required** — The
//!   name of the repository for which the trigger is configured.
//!
//! ## Output
//!
//! * `configurationId` (RepositoryTriggersConfigurationId) — The system-generated unique ID
//!   for the trigger.
//! * `triggers` (RepositoryTriggersList) — The JSON block of configuration information for
//!   each trigger.
//!
//! ## Errors
//!
//! * `EncryptionIntegrityChecksFailedException` (HTTP 500) — An encryption integrity check
//!   failed.
//! * `EncryptionKeyAccessDeniedException` (HTTP 400) — An encryption key could not be
//!   accessed.
//! * `EncryptionKeyDisabledException` (HTTP 400) — The encryption key is disabled.
//! * `EncryptionKeyNotFoundException` (HTTP 400) — No encryption key was found.
//! * `EncryptionKeyUnavailableException` (HTTP 400) — The encryption key is not available.
//! * `InvalidRepositoryNameException` (HTTP 400) — A specified repository name is not valid.
//!   This exception occurs only when a specified repository name is not valid. Other
//!   exceptions occur when a required repository parameter is missing, or when a specified
//!   repository does not exist.
//! * `RepositoryDoesNotExistException` (HTTP 400) — The specified repository does not exist.
//! * `RepositoryNameRequiredException` (HTTP 400) — A repository name is required, but was
//!   not specified.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetRepositoryTriggersError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codecommit-codecommit-20150413.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetRepositoryTriggersError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.codecommit#GetRepositoryTriggers")]
#[derive(Debug)]
pub enum GetRepositoryTriggersErrorModeled {
    EncryptionIntegrityChecksFailedException(EncryptionIntegrityChecksFailedException),
    EncryptionKeyAccessDeniedException(EncryptionKeyAccessDeniedException),
    EncryptionKeyDisabledException(EncryptionKeyDisabledException),
    EncryptionKeyNotFoundException(EncryptionKeyNotFoundException),
    EncryptionKeyUnavailableException(EncryptionKeyUnavailableException),
    InvalidRepositoryNameException(InvalidRepositoryNameException),
    RepositoryDoesNotExistException(RepositoryDoesNotExistException),
    RepositoryNameRequiredException(RepositoryNameRequiredException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_repository_triggers::GetRepositoryTriggersError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetRepositoryTriggersError = crate::scaffolded::Scaffolded<GetRepositoryTriggersErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetRepositoryTriggersErrorModeled> for GetRepositoryTriggersError {
    fn from(modeled: GetRepositoryTriggersErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetRepositoryTriggers` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetRepositoryTriggers<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetRepositoryTriggers<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetRepositoryTriggers<Eng>
{
    type Input = GetRepositoryTriggersInput;
    type Output = GetRepositoryTriggersOutput;
    type Error = GetRepositoryTriggersError;

    const NAME: &'static str = "GetRepositoryTriggers";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_REPOSITORY_TRIGGERS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_REPOSITORY_TRIGGERS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // A read of stored state alone: nothing about it comes from the request
        // beyond the repository it names.
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            // An unreachable store is an internal mounting fault, not a modelled
            // CodeCommit failure; every handler in this crate refuses it this way.
            crate::scaffolded::Scaffolded::validation("the repository store is unavailable")
        })?;
        let store = state.lock();
        let repository = store.repositories.get(&input.repository_name).ok_or_else(|| {
            GetRepositoryTriggersErrorModeled::RepositoryDoesNotExistException(
                RepositoryDoesNotExistException {
                    message: Some(crate::state::repository_does_not_exist_message(
                        &input.repository_name,
                    )),
                },
            )
        })?;

        let configuration = repository.trigger_configuration.as_ref();
        Ok(GetRepositoryTriggersOutput {
            configuration_id: configuration
                .map(|configuration| configuration.configuration_id.clone()),
            // The triggers of a repository nobody has configured are NONE of them,
            // reported as the empty list rather than withheld: the member is the
            // configuration's trigger list, and that list is empty.
            triggers: Some(
                configuration
                    .map(|configuration| configuration.triggers.clone())
                    .unwrap_or_default(),
            ),
        })
    }
}
