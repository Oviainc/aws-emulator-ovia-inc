// SPDX-License-Identifier: CC0-1.0

//! @op GetTieringConfiguration
//!
//! Reports the stored tiering configuration the `TieringConfigurationName`
//! addresses, projected through the ONE single-configuration read projection
//! ([`crate::state::TieringConfigurationRecord::described`]), so this read and
//! the create that stored it can never disagree.
//!
//! A name the account holds no configuration under — one never created, or one
//! a delete has removed — is the one not-found refusal
//! ([`crate::state::tiering_configuration_not_found`]), shared with the delete.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetTieringConfiguration`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
//! ```
//!
//! Returns TieringConfiguration details for the specified TieringConfigurationName. The
//! details are the body of a tiering configuration in JSON format, in addition to
//! configuration metadata.
//!
//! ## Input
//!
//! * `TieringConfigurationName` (TieringConfigurationName, pattern ^[a-zA-Z0-9_]{1,200}$)
//!   **required** — The unique name of a tiering configuration.
//!
//! ## Output
//!
//! * `TieringConfiguration` (TieringConfiguration) — Specifies the body of a tiering
//!   configuration. Includes TieringConfigurationName.
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
// `GetTieringConfigurationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetTieringConfigurationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.backup#GetTieringConfiguration")]
#[derive(Debug)]
pub enum GetTieringConfigurationErrorModeled {
    InvalidParameterValueException(InvalidParameterValueException),
    MissingParameterValueException(MissingParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_tiering_configuration::GetTieringConfigurationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetTieringConfigurationError = crate::scaffolded::Scaffolded<GetTieringConfigurationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetTieringConfigurationErrorModeled> for GetTieringConfigurationError {
    fn from(modeled: GetTieringConfigurationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetTieringConfiguration` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetTieringConfiguration<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetTieringConfiguration<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetTieringConfiguration<Eng>
{
    type Input = GetTieringConfigurationInput;
    type Output = GetTieringConfigurationOutput;
    type Error = GetTieringConfigurationError;

    const NAME: &'static str = "GetTieringConfiguration";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_TIERING_CONFIGURATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_TIERING_CONFIGURATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetTieringConfigurationErrorModeled::ServiceUnavailableException(
                ServiceUnavailableException {
                    message: Some("the backup vault store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;
        let store = state.lock();
        // A pure read: a tiering configuration moves through no states, so this
        // call has no transition to materialise and writes nothing.
        let configuration = store
            .stored_tiering_configuration(&input.tiering_configuration_name)
            .map_err(GetTieringConfigurationErrorModeled::ResourceNotFoundException)?;
        Ok(GetTieringConfigurationOutput {
            tiering_configuration: Some(configuration.described()),
        })
    }
}
