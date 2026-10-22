// SPDX-License-Identifier: CC0-1.0

//! @op UpdateFileSystemProtection
//!
//! Sets one file system's replication overwrite protection, which is what decides
//! whether it may be the destination of a replication configuration.
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
//! # Modeled contract for `UpdateFileSystemProtection`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-efs-magnolioapiservice-v20150201/smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json")
//! ```
//!
//! Updates protection on the file system. This operation requires permissions for the
//! elasticfilesystem:UpdateFileSystemProtection action.
//!
//! ## Input
//!
//! * `FileSystemId` (FileSystemId, len 0..128, pattern
//!   ^(arn:aws[-a-z]*:elasticfilesystem:[0-9a-z-:]+:file-system/fs-[0-9a-f]{8,40}|fs-[0-9a-f]{8,40})$)
//!   **required** — The ID of the file system to update.
//! * `ReplicationOverwriteProtection` (ReplicationOverwriteProtection, one of: DISABLED |
//!   ENABLED | REPLICATING) — The status of the file system's replication overwrite
//!   protection. ENABLED – The file system cannot be used as the destination file system in a
//!   replication configuration. The file system is writeable. Replication overwrite
//!   protection is ENABLED by default. DISABLED – The file system can be used as the
//!   destination file system in a replication configuration. The file system is read-only and
//!   can only be modified by EFS replication. REPLICATING – The file system is being used as
//!   the destination file system in a replication configuration. The file system is read-only
//!   and is only modified only by EFS replication. If the replication configuration is
//!   deleted, the file system's replication overwrite …
//!
//! ## Output
//!
//! * `ReplicationOverwriteProtection` (ReplicationOverwriteProtection, one of: DISABLED |
//!   ENABLED | REPLICATING) — The status of the file system's replication overwrite
//!   protection. ENABLED – The file system cannot be used as the destination file system in a
//!   replication configuration. The file system is writeable. Replication overwrite
//!   protection is ENABLED by default. DISABLED – The file system can be used as the
//!   destination file system in a replication configuration. The file system is read-only and
//!   can only be modified by EFS replication. REPLICATING – The file system is being used as
//!   the destination file system in a replication configuration. The file system is read-only
//!   and is modified only by EFS replication. If the replication configuration is deleted,
//!   the file system's replication overwrite …
//!
//! ## Errors
//!
//! * `BadRequest` (HTTP 400) — Returned if the request is malformed or contains an error such
//!   as an invalid parameter value or a missing required parameter.
//! * `FileSystemNotFound` (HTTP 404) — Returned if the specified FileSystemId value doesn't
//!   exist in the requester's Amazon Web Services account.
//! * `IncorrectFileSystemLifeCycleState` (HTTP 409) — Returned if the file system's lifecycle
//!   state is not "available".
//! * `InsufficientThroughputCapacity` (HTTP 503) — Returned if there's not enough capacity to
//!   provision additional throughput. This value might be returned when you try to create a
//!   file system in provisioned throughput mode, when you attempt to increase the provisioned
//!   throughput of an existing file system, or when you attempt to change an existing file
//!   system from Bursting Throughput to Provisioned Throughput mode. Try again later.
//! * `InternalServerError` (HTTP 500) — Returned if an error occurred on the server side.
//! * `ReplicationAlreadyExists` (HTTP 409) — Returned if the file system is already included
//!   in a replication configuration.>
//! * `ThroughputLimitExceeded` (HTTP 400) — Returned if the throughput mode or amount of
//!   provisioned throughput can't be changed because the throughput limit of 1024 MiB/s has
//!   been reached.
//! * `TooManyRequests` (HTTP 429) — Returned if you don’t wait at least 24 hours before
//!   either changing the throughput mode, or decreasing the Provisioned Throughput value.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateFileSystemProtectionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateFileSystemProtectionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.efs#UpdateFileSystemProtection")]
#[derive(Debug)]
pub enum UpdateFileSystemProtectionErrorModeled {
    BadRequest(BadRequest),
    FileSystemNotFound(FileSystemNotFound),
    IncorrectFileSystemLifeCycleState(IncorrectFileSystemLifeCycleState),
    InsufficientThroughputCapacity(InsufficientThroughputCapacity),
    InternalServerError(InternalServerError),
    ReplicationAlreadyExists(ReplicationAlreadyExists),
    ThroughputLimitExceeded(ThroughputLimitExceeded),
    TooManyRequests(TooManyRequests),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_file_system_protection::UpdateFileSystemProtectionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateFileSystemProtectionError = crate::scaffolded::Scaffolded<UpdateFileSystemProtectionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateFileSystemProtectionErrorModeled> for UpdateFileSystemProtectionError {
    fn from(modeled: UpdateFileSystemProtectionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateFileSystemProtection` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateFileSystemProtection<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateFileSystemProtection<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateFileSystemProtection<Eng>
{
    type Input = UpdateFileSystemProtectionInput;
    type Output = UpdateFileSystemProtectionOutput;
    type Error = UpdateFileSystemProtectionError;

    const NAME: &'static str = "UpdateFileSystemProtection";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_FILE_SYSTEM_PROTECTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_FILE_SYSTEM_PROTECTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

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

        // "The ID of the file system to update" — the member accepts an id or an ARN.
        let file_system_id = efs::file_system_id_of(&input.file_system_id).to_string();
        let Some(file_system) = state.file_systems.get(&file_system_id) else {
            return Err(UpdateFileSystemProtectionErrorModeled::FileSystemNotFound(
                FileSystemNotFound {
                    error_code: "FileSystemNotFound".to_string(),
                    message: Some(crate::state::file_system_not_found_message(
                        &file_system_id,
                    )),
                },
            )
            .into());
        };
        if file_system.life_cycle_state != efs::LIFE_CYCLE_AVAILABLE {
            return Err(
                UpdateFileSystemProtectionErrorModeled::IncorrectFileSystemLifeCycleState(
                    IncorrectFileSystemLifeCycleState {
                        error_code: "IncorrectFileSystemLifeCycleState".to_string(),
                        message: Some(format!(
                            "File system {} is in life cycle state {}, not {}",
                            efs::quoted(&file_system_id),
                            file_system.life_cycle_state,
                            efs::LIFE_CYCLE_AVAILABLE
                        )),
                    },
                )
                .into(),
            );
        }

        match input.replication_overwrite_protection.as_deref() {
            // "REPLICATING – The file system is being used as the destination file
            // system in a replication configuration": EFS sets that status itself when a
            // replication configuration names the file system as its destination, so a
            // caller cannot ask for it.
            Some(requested) if requested == efs::PROTECTION_REPLICATING => {
                return Err(bad_request(format!(
                    "ReplicationOverwriteProtection {} is set by Amazon EFS when a replication \
                     configuration names this file system as its destination; specify {} or {}",
                    efs::PROTECTION_REPLICATING,
                    efs::PROTECTION_ENABLED,
                    efs::PROTECTION_DISABLED
                )));
            }
            Some(requested) => {
                // "Returned if the file system is already included in a replication
                // configuration": while it is, its protection follows that configuration
                // and a caller cannot move it.
                if let Some(replication) = state.replication_of(&file_system_id) {
                    let source = replication.source_file_system_id.clone();
                    return Err(
                        UpdateFileSystemProtectionErrorModeled::ReplicationAlreadyExists(
                            ReplicationAlreadyExists {
                                error_code: Some("ReplicationAlreadyExists".to_string()),
                                message: Some(format!(
                                    "File system {} is already part of the replication \
                                     configuration of source file system {}",
                                    efs::quoted(&file_system_id),
                                    efs::quoted(&source)
                                )),
                            },
                        )
                        .into(),
                    );
                }
                // The modelled enum closure is already refused at the boundary, so a
                // value reaching here is one the enum declares.
                if let Some(file_system) = state.file_systems.get_mut(&file_system_id) {
                    file_system.replication_overwrite_protection = requested.to_string();
                }
            }
            // The member is optional, so a request that names no status leaves the file
            // system as it is and reports what it carries.
            None => {}
        }

        let current = state
            .file_systems
            .get(&file_system_id)
            .map(|file_system| file_system.replication_overwrite_protection.clone());
        Ok(UpdateFileSystemProtectionOutput {
            replication_overwrite_protection: current,
        })
    }
}

/// A 400 `BadRequest`: "the request is malformed or contains an error such as an invalid
/// parameter value or a missing required parameter".
fn bad_request(message: impl Into<String>) -> UpdateFileSystemProtectionError {
    UpdateFileSystemProtectionErrorModeled::BadRequest(BadRequest {
        error_code: "BadRequest".to_string(),
        message: Some(message.into()),
    })
    .into()
}

/// A 500 `InternalServerError`, for the one case this handler cannot serve: the engine
/// this service was mounted on holds no store for it.
fn internal_server_error() -> UpdateFileSystemProtectionError {
    UpdateFileSystemProtectionErrorModeled::InternalServerError(InternalServerError {
        error_code: "InternalServerError".to_string(),
        message: Some("the EFS emulator state store is unavailable".to_string()),
    })
    .into()
}
