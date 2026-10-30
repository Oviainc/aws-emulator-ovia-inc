// SPDX-License-Identifier: CC0-1.0

//! @op PutRepositoryTriggers
//!
//! "Replaces all triggers for a repository. Used to create or delete triggers": the
//! list a caller sends IS the repository's trigger configuration afterwards, so the
//! stored configuration is replaced whole rather than merged, and a request carrying
//! an empty list is how a caller deletes every trigger. Each write mints the
//! `configurationId` it returns, and `GetRepositoryTriggers` reports that id with the
//! triggers the write stored.
//!
//! A configuration is judged by [`crate::state::trigger_configuration_problem`], the
//! one place this crate decides which configurations a repository can hold, so the
//! operations that take a `RepositoryTriggersList` cannot disagree about it; every
//! problem it names is answered with that member's modelled error, in [`refusal`].
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `PutRepositoryTriggers`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codecommit-codecommit-20150413/smithy-model/com-amazonaws-codecommit-codecommit-20150413.json")
//! ```
//!
//! Replaces all triggers for a repository. Used to create or delete triggers.
//!
//! ## Input
//!
//! * `repositoryName` (RepositoryName, len 1..100, pattern ^[\w\.-]+$) **required** — The
//!   name of the repository where you want to create or update the trigger.
//! * `triggers` (RepositoryTriggersList) **required** — The JSON block of configuration
//!   information for each trigger.
//!
//! ## Output
//!
//! * `configurationId` (RepositoryTriggersConfigurationId) — The system-generated unique ID
//!   for the create or update operation.
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
//! * `InvalidRepositoryTriggerBranchNameException` (HTTP 400) — One or more branch names
//!   specified for the trigger is not valid.
//! * `InvalidRepositoryTriggerCustomDataException` (HTTP 400) — The custom data provided for
//!   the trigger is not valid.
//! * `InvalidRepositoryTriggerDestinationArnException` (HTTP 400) — The Amazon Resource Name
//!   (ARN) for the trigger is not valid for the specified destination. The most common reason
//!   for this error is that the ARN does not meet the requirements for the service type.
//! * `InvalidRepositoryTriggerEventsException` (HTTP 400) — One or more events specified for
//!   the trigger is not valid. Check to make sure that all events specified match the
//!   requirements for allowed events.
//! * `InvalidRepositoryTriggerNameException` (HTTP 400) — The name of the trigger is not
//!   valid.
//! * `InvalidRepositoryTriggerRegionException` (HTTP 400) — The Amazon Web Services Region
//!   for the trigger target does not match the Amazon Web Services Region for the repository.
//!   Triggers must be created in the same Amazon Web Services Region as the target for the
//!   trigger.
//! * `MaximumBranchesExceededException` (HTTP 400) — The number of branches for the trigger
//!   was exceeded.
//! * `MaximumRepositoryTriggersExceededException` (HTTP 400) — The number of triggers allowed
//!   for the repository was exceeded.
//! * `RepositoryDoesNotExistException` (HTTP 400) — The specified repository does not exist.
//! * `RepositoryNameRequiredException` (HTTP 400) — A repository name is required, but was
//!   not specified.
//! * `RepositoryTriggerBranchNameListRequiredException` (HTTP 400) — At least one branch name
//!   is required, but was not specified in the trigger configuration.
//! * `RepositoryTriggerDestinationArnRequiredException` (HTTP 400) — A destination ARN for
//!   the target service for the trigger is required, but was not specified.
//! * `RepositoryTriggerEventsListRequiredException` (HTTP 400) — At least one event for the
//!   trigger is required, but was not specified.
//! * `RepositoryTriggerNameRequiredException` (HTTP 400) — A name for the trigger is
//!   required, but was not specified.
//! * `RepositoryTriggersListRequiredException` (HTTP 400) — The list of triggers for the
//!   repository is required, but was not specified.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `PutRepositoryTriggersError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codecommit-codecommit-20150413.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PutRepositoryTriggersError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.codecommit#PutRepositoryTriggers")]
#[derive(Debug)]
pub enum PutRepositoryTriggersErrorModeled {
    EncryptionIntegrityChecksFailedException(EncryptionIntegrityChecksFailedException),
    EncryptionKeyAccessDeniedException(EncryptionKeyAccessDeniedException),
    EncryptionKeyDisabledException(EncryptionKeyDisabledException),
    EncryptionKeyNotFoundException(EncryptionKeyNotFoundException),
    EncryptionKeyUnavailableException(EncryptionKeyUnavailableException),
    InvalidRepositoryNameException(InvalidRepositoryNameException),
    InvalidRepositoryTriggerBranchNameException(InvalidRepositoryTriggerBranchNameException),
    InvalidRepositoryTriggerCustomDataException(InvalidRepositoryTriggerCustomDataException),
    InvalidRepositoryTriggerDestinationArnException(InvalidRepositoryTriggerDestinationArnException),
    InvalidRepositoryTriggerEventsException(InvalidRepositoryTriggerEventsException),
    InvalidRepositoryTriggerNameException(InvalidRepositoryTriggerNameException),
    InvalidRepositoryTriggerRegionException(InvalidRepositoryTriggerRegionException),
    MaximumBranchesExceededException(MaximumBranchesExceededException),
    MaximumRepositoryTriggersExceededException(MaximumRepositoryTriggersExceededException),
    RepositoryDoesNotExistException(RepositoryDoesNotExistException),
    RepositoryNameRequiredException(RepositoryNameRequiredException),
    RepositoryTriggerBranchNameListRequiredException(RepositoryTriggerBranchNameListRequiredException),
    RepositoryTriggerDestinationArnRequiredException(RepositoryTriggerDestinationArnRequiredException),
    RepositoryTriggerEventsListRequiredException(RepositoryTriggerEventsListRequiredException),
    RepositoryTriggerNameRequiredException(RepositoryTriggerNameRequiredException),
    RepositoryTriggersListRequiredException(RepositoryTriggersListRequiredException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_repository_triggers::PutRepositoryTriggersError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutRepositoryTriggersError = crate::scaffolded::Scaffolded<PutRepositoryTriggersErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutRepositoryTriggersErrorModeled> for PutRepositoryTriggersError {
    fn from(modeled: PutRepositoryTriggersErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutRepositoryTriggers` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutRepositoryTriggers<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutRepositoryTriggers<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The modelled error for a trigger configuration this repository cannot hold.
///
/// [`crate::state::TriggerProblem`] names the member at fault; this is the one
/// place this operation turns each into the error the model declares for that
/// member, with the wording the problem itself carries.
fn refusal(problem: crate::state::TriggerProblem) -> PutRepositoryTriggersError {
    use crate::state::TriggerProblem;
    let message = Some(problem.message());
    match problem {
        TriggerProblem::TooManyTriggers => {
            PutRepositoryTriggersErrorModeled::MaximumRepositoryTriggersExceededException(
                MaximumRepositoryTriggersExceededException { message },
            )
        }
        TriggerProblem::NameMissing => {
            PutRepositoryTriggersErrorModeled::RepositoryTriggerNameRequiredException(
                RepositoryTriggerNameRequiredException { message },
            )
        }
        TriggerProblem::NameInvalid | TriggerProblem::NameNotUnique => {
            PutRepositoryTriggersErrorModeled::InvalidRepositoryTriggerNameException(
                InvalidRepositoryTriggerNameException { message },
            )
        }
        TriggerProblem::DestinationArnMissing => {
            PutRepositoryTriggersErrorModeled::RepositoryTriggerDestinationArnRequiredException(
                RepositoryTriggerDestinationArnRequiredException { message },
            )
        }
        TriggerProblem::DestinationArnInvalid => {
            PutRepositoryTriggersErrorModeled::InvalidRepositoryTriggerDestinationArnException(
                InvalidRepositoryTriggerDestinationArnException { message },
            )
        }
        TriggerProblem::DestinationRegion => {
            PutRepositoryTriggersErrorModeled::InvalidRepositoryTriggerRegionException(
                InvalidRepositoryTriggerRegionException { message },
            )
        }
        TriggerProblem::EventsMissing => {
            PutRepositoryTriggersErrorModeled::RepositoryTriggerEventsListRequiredException(
                RepositoryTriggerEventsListRequiredException { message },
            )
        }
        TriggerProblem::EventsInvalid | TriggerProblem::EventsAllCombined => {
            PutRepositoryTriggersErrorModeled::InvalidRepositoryTriggerEventsException(
                InvalidRepositoryTriggerEventsException { message },
            )
        }
        TriggerProblem::TooManyBranches => {
            PutRepositoryTriggersErrorModeled::MaximumBranchesExceededException(
                MaximumBranchesExceededException { message },
            )
        }
        TriggerProblem::BranchNameInvalid => {
            PutRepositoryTriggersErrorModeled::InvalidRepositoryTriggerBranchNameException(
                InvalidRepositoryTriggerBranchNameException { message },
            )
        }
        TriggerProblem::CustomDataInvalid => {
            PutRepositoryTriggersErrorModeled::InvalidRepositoryTriggerCustomDataException(
                InvalidRepositoryTriggerCustomDataException { message },
            )
        }
    }
    .into()
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutRepositoryTriggers<Eng>
{
    type Input = PutRepositoryTriggersInput;
    type Output = PutRepositoryTriggersOutput;
    type Error = PutRepositoryTriggersError;

    const NAME: &'static str = "PutRepositoryTriggers";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_REPOSITORY_TRIGGERS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_REPOSITORY_TRIGGERS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Nothing about a trigger configuration depends on the request beyond its
        // payload: the Region a destination must match is the REPOSITORY's, held on
        // the repository since it was created.
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            // An unreachable store is an internal mounting fault, not a modelled
            // CodeCommit failure; every handler in this crate refuses it this way.
            crate::scaffolded::Scaffolded::validation("the repository store is unavailable")
        })?;
        let mut store = state.lock();
        let repository = store
            .repositories
            .get_mut(&input.repository_name)
            .ok_or_else(|| {
                PutRepositoryTriggersErrorModeled::RepositoryDoesNotExistException(
                    RepositoryDoesNotExistException {
                        message: Some(crate::state::repository_does_not_exist_message(
                            &input.repository_name,
                        )),
                    },
                )
            })?;

        // The configuration is judged BEFORE anything is stored, so a trigger the
        // repository cannot hold leaves the configuration it already has in place.
        if let Some(problem) =
            crate::state::trigger_configuration_problem(&input.triggers, &repository.region)
        {
            return Err(refusal(problem));
        }

        // "Replaces all triggers": the list the caller sent IS the configuration
        // from here on, and the id this write mints is what identifies it.
        let configuration_id = crate::state::trigger_configuration_id(
            &input.repository_name,
            std::time::SystemTime::now(),
        );
        repository.trigger_configuration = Some(crate::state::TriggerConfiguration {
            configuration_id: configuration_id.clone(),
            triggers: input.triggers,
        });

        Ok(PutRepositoryTriggersOutput {
            configuration_id: Some(configuration_id),
        })
    }
}
