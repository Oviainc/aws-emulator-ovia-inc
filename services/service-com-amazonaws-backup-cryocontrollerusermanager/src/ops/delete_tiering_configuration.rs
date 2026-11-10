// SPDX-License-Identifier: CC0-1.0

//! @op DeleteTieringConfiguration
//!
//! Removes the stored tiering configuration the `TieringConfigurationName`
//! addresses from [`crate::state`], so every later read of that name reports it
//! absent and the listing stops paging over it.
//!
//! The name is resolved by the step this delete shares with the read
//! ([`crate::state::ComAmazonawsBackupCryocontrollerusermanagerState::stored_tiering_configuration`]),
//! so a name the account holds no configuration under — one never created, or
//! one this delete has already removed — is refused with the one not-found
//! sentence rather than answered as a delete of nothing.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteTieringConfiguration`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
//! ```
//!
//! Deletes the tiering configuration specified by a tiering configuration name.
//!
//! ## Input
//!
//! * `TieringConfigurationName` (TieringConfigurationName, pattern ^[a-zA-Z0-9_]{1,200}$)
//!   **required** — The unique name of a tiering configuration.
//!
//! ## Errors
//!
//! * `InvalidParameterValueException` (HTTP 400) — Indicates that something is wrong with a
//!   parameter's value. For example, the value is out of range.
//! * `MissingParameterValueException` (HTTP 400) — Indicates that a required parameter is
//!   missing.
//! * `ResourceNotFoundException` (HTTP 400) — A resource that is required for the action
//!   doesn't exist.
//! * `ServiceUnavailableException` (HTTP 500) — The request failed due to a temporary failure
//!   of the server.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteTieringConfigurationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteTieringConfigurationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.backup#DeleteTieringConfiguration")]
#[derive(Debug)]
pub enum DeleteTieringConfigurationErrorModeled {
    InvalidParameterValueException(InvalidParameterValueException),
    MissingParameterValueException(MissingParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_tiering_configuration::DeleteTieringConfigurationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteTieringConfigurationError = crate::scaffolded::Scaffolded<DeleteTieringConfigurationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteTieringConfigurationErrorModeled> for DeleteTieringConfigurationError {
    fn from(modeled: DeleteTieringConfigurationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteTieringConfiguration` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteTieringConfiguration<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteTieringConfiguration<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteTieringConfiguration<Eng>
{
    type Input = DeleteTieringConfigurationInput;
    type Output = DeleteTieringConfigurationOutput;
    type Error = DeleteTieringConfigurationError;

    const NAME: &'static str = "DeleteTieringConfiguration";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_TIERING_CONFIGURATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_TIERING_CONFIGURATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteTieringConfigurationErrorModeled::ServiceUnavailableException(
                ServiceUnavailableException {
                    message: Some("the backup vault store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;
        let mut store = state.lock();
        // Resolved BEFORE anything is removed, so a refused request writes
        // nothing and a second delete of the same name answers the same
        // refusal as a delete of a name nothing ever created.
        store
            .stored_tiering_configuration(&input.tiering_configuration_name)
            .map_err(DeleteTieringConfigurationErrorModeled::ResourceNotFoundException)?;
        store
            .tiering_configurations
            .remove(&input.tiering_configuration_name);
        Ok(DeleteTieringConfigurationOutput {})
    }
}
