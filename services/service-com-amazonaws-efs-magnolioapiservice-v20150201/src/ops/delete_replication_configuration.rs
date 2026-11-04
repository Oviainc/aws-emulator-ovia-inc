// SPDX-License-Identifier: CC0-1.0

//! @op DeleteReplicationConfiguration
//!
//! Deletes one source file system's replication configuration and re-enables the
//! destination file system's replication overwrite protection, which is what makes that
//! file system writeable again.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteReplicationConfiguration`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-efs-magnolioapiservice-v20150201/smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json")
//! ```
//!
//! Deletes a replication configuration. Deleting a replication configuration ends the
//! replication process. After a replication configuration is deleted, the destination file
//! system becomes Writeable and its replication overwrite protection is re-enabled. For
//! more information, see Delete a replication configuration. This operation requires
//! permissions for the elasticfilesystem:DeleteReplicationConfiguration action.
//!
//! ## Input
//!
//! * `DeletionMode` (DeletionMode, one of: ALL_CONFIGURATIONS | LOCAL_CONFIGURATION_ONLY) —
//!   When replicating across Amazon Web Services accounts or across Amazon Web Services
//!   Regions, Amazon EFS deletes the replication configuration from both the source and
//!   destination account or Region (ALL_CONFIGURATIONS) by default. If there's a
//!   configuration or permissions issue that prevents Amazon EFS from deleting the
//!   replication configuration from both sides, you can use the LOCAL_CONFIGURATION_ONLY mode
//!   to delete the replication configuration from only the local side (the account or Region
//!   from which the delete is performed). Only use the LOCAL_CONFIGURATION_ONLY mode in the
//!   case that Amazon EFS is unable to delete the replication configuration in both the
//!   source and destination account …
//! * `SourceFileSystemId` (FileSystemId, len 0..128, pattern
//!   ^(arn:aws[-a-z]*:elasticfilesystem:[0-9a-z-:]+:file-system/fs-[0-9a-f]{8,40}|fs-[0-9a-f]{8,40})$)
//!   **required** — The ID of the source file system in the replication configuration.
//!
//! ## Errors
//!
//! * `BadRequest` (HTTP 400) — Returned if the request is malformed or contains an error such
//!   as an invalid parameter value or a missing required parameter.
//! * `FileSystemNotFound` (HTTP 404) — Returned if the specified FileSystemId value doesn't
//!   exist in the requester's Amazon Web Services account.
//! * `InternalServerError` (HTTP 500) — Returned if an error occurred on the server side.
//! * `ReplicationNotFound` (HTTP 404) — Returned if the specified file system does not have a
//!   replication configuration.

#[allow(unused_imports)]
use super::types::*;

// This operation's not-found refusals carry the SENTENCE ALONE, with the error shape
// named in `x-amzn-errortype` and no `ErrorCode` member in the body. These explicit
// imports shadow the glob above, so the two variants below take that body instead of
// the generated shape; the rule lives on `crate::service::wire::MessageOnly`.
use crate::service::wire::{FileSystemNotFound, ReplicationNotFound};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteReplicationConfigurationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json");

#[derive(Debug, Clone, Default)]
pub struct DeleteReplicationConfigurationOutput;

impl DeleteReplicationConfigurationOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "DELETE",
        "/2015-02-01/file-systems/{SourceFileSystemId}/replication-configuration",
        Some(204u16),
    ));
}

pub static DELETE_REPLICATION_CONFIGURATION_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &DeleteReplicationConfigurationOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DeleteReplicationConfigurationOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteReplicationConfigurationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.efs#DeleteReplicationConfiguration")]
#[derive(Debug)]
pub enum DeleteReplicationConfigurationErrorModeled {
    BadRequest(BadRequest),
    FileSystemNotFound(FileSystemNotFound),
    InternalServerError(InternalServerError),
    ReplicationNotFound(ReplicationNotFound),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_replication_configuration::DeleteReplicationConfigurationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteReplicationConfigurationError = crate::scaffolded::Scaffolded<DeleteReplicationConfigurationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteReplicationConfigurationErrorModeled> for DeleteReplicationConfigurationError {
    fn from(modeled: DeleteReplicationConfigurationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteReplicationConfiguration` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteReplicationConfiguration<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteReplicationConfiguration<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteReplicationConfiguration<Eng>
{
    type Input = DeleteReplicationConfigurationInput;
    type Output = DeleteReplicationConfigurationOutput;
    type Error = DeleteReplicationConfigurationError;

    const NAME: &'static str = "DeleteReplicationConfiguration";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_REPLICATION_CONFIGURATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = DELETE_REPLICATION_CONFIGURATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 204u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state as efs;

        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(internal_server_error());
        };
        let now = ::std::time::SystemTime::now();

        let mut state = state.lock();
        state.materialize_due_transitions(now);

        // "The ID of the source file system in the replication configuration" — the
        // member accepts an id or an ARN.
        let source_id = efs::file_system_id_of(&input.source_file_system_id).to_string();
        if !state.file_systems.contains_key(&source_id) {
            return Err(file_system_not_found(&source_id));
        }
        // `DeletionMode` chooses whether the configuration is deleted "from both the
        // source and destination account or Region (ALL_CONFIGURATIONS)" or "from only
        // the local side": every file system this emulator holds is in the one local
        // account, so both modes remove the same single configuration.
        let Some(replication) = state.replications.remove(&source_id) else {
            return Err(replication_not_found(&source_id));
        };
        // "After a replication configuration is deleted, the destination file system
        // becomes Writeable and its replication overwrite protection is re-enabled."
        for destination in &replication.destinations {
            if let Some(file_system) = state.file_systems.get_mut(&destination.file_system_id) {
                file_system.replication_overwrite_protection =
                    efs::PROTECTION_ENABLED.to_string();
            }
        }
        Ok(DeleteReplicationConfigurationOutput)
    }
}

/// A 404 `FileSystemNotFound`: "the specified FileSystemId value doesn't exist in the
/// requester's Amazon Web Services account", with the one sentence every entry point
/// that resolves a `FileSystemId` renders.
fn file_system_not_found(file_system_id: &str) -> DeleteReplicationConfigurationError {
    DeleteReplicationConfigurationErrorModeled::FileSystemNotFound(FileSystemNotFound::new(
        crate::state::file_system_not_found_message(file_system_id),
    ))
    .into()
}

/// A 404 `ReplicationNotFound`: "the specified file system does not have a replication
/// configuration" — this delete's refusal names the source file system whose
/// configuration it could not find, through
/// [`crate::state::replication_configuration_not_found_message`].
fn replication_not_found(file_system_id: &str) -> DeleteReplicationConfigurationError {
    DeleteReplicationConfigurationErrorModeled::ReplicationNotFound(ReplicationNotFound::new(
        crate::state::replication_configuration_not_found_message(file_system_id),
    ))
    .into()
}

/// A 500 `InternalServerError`, for the one case this handler cannot serve: the engine
/// this service was mounted on holds no store for it.
fn internal_server_error() -> DeleteReplicationConfigurationError {
    DeleteReplicationConfigurationErrorModeled::InternalServerError(InternalServerError {
        error_code: "InternalServerError".to_string(),
        message: Some("the EFS emulator state store is unavailable".to_string()),
    })
    .into()
}
