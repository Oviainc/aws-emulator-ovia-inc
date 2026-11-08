// SPDX-License-Identifier: CC0-1.0

//! @op PutBackupPolicy
//!
//! Starts or stops automatic backups of one file system: the call is an ACCEPTANCE, so it
//! sets the file system's backup policy to the status EFS reports while it turns backups
//! on or off, and the clock settles that status at the one the caller asked for.
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
//! # Modeled contract for `PutBackupPolicy`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-efs-magnolioapiservice-v20150201/smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json")
//! ```
//!
//! Updates the file system's backup policy. Use this action to start or stop automatic
//! backups of the file system.
//!
//! ## Input
//!
//! * `BackupPolicy` (BackupPolicy) **required** — The backup policy included in the
//!   PutBackupPolicy request.
//! * `FileSystemId` (FileSystemId, len 0..128, pattern
//!   ^(arn:aws[-a-z]*:elasticfilesystem:[0-9a-z-:]+:file-system/fs-[0-9a-f]{8,40}|fs-[0-9a-f]{8,40})$)
//!   **required** — Specifies which EFS file system to update the backup policy for.
//!
//! ## Output
//!
//! * `BackupPolicy` (BackupPolicy) — Describes the file system's backup policy, indicating
//!   whether automatic backups are turned on or off.
//!
//! ## Errors
//!
//! * `BadRequest` (HTTP 400) — Returned if the request is malformed or contains an error such
//!   as an invalid parameter value or a missing required parameter.
//! * `FileSystemNotFound` (HTTP 404) — Returned if the specified FileSystemId value doesn't
//!   exist in the requester's Amazon Web Services account.
//! * `IncorrectFileSystemLifeCycleState` (HTTP 409) — Returned if the file system's lifecycle
//!   state is not "available".
//! * `InternalServerError` (HTTP 500) — Returned if an error occurred on the server side.
//! * `ValidationException` (HTTP 400) — Returned if the Backup service is not available in
//!   the Amazon Web Services Region in which the request was made.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `PutBackupPolicyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PutBackupPolicyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.efs#PutBackupPolicy")]
#[derive(Debug)]
pub enum PutBackupPolicyErrorModeled {
    BadRequest(BadRequest),
    FileSystemNotFound(FileSystemNotFound),
    IncorrectFileSystemLifeCycleState(IncorrectFileSystemLifeCycleState),
    InternalServerError(InternalServerError),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_backup_policy::PutBackupPolicyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutBackupPolicyError = crate::scaffolded::Scaffolded<PutBackupPolicyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutBackupPolicyErrorModeled> for PutBackupPolicyError {
    fn from(modeled: PutBackupPolicyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutBackupPolicy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutBackupPolicy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutBackupPolicy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutBackupPolicy<Eng>
{
    type Input = PutBackupPolicyInput;
    type Output = PutBackupPolicyOutput;
    type Error = PutBackupPolicyError;

    const NAME: &'static str = "PutBackupPolicy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_BACKUP_POLICY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_BACKUP_POLICY_OUTPUT_SCHEMA;
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

        // "Specifies which EFS file system to update the backup policy for": the member
        // accepts the id or the ARN of the file system.
        let file_system_id = efs::file_system_id_of(&input.file_system_id).to_string();
        let Some(file_system) = state.file_systems.get(&file_system_id) else {
            return Err(not_found(efs::file_system_not_found_message(
                &file_system_id,
            )));
        };
        // `IncorrectFileSystemLifeCycleState`: "Returned if the file system's lifecycle
        // state is not available."
        if file_system.life_cycle_state != efs::LIFE_CYCLE_AVAILABLE {
            return Err(incorrect_life_cycle_state(format!(
                "File system {} is in life cycle state {}, not {}",
                efs::quoted(&file_system_id),
                file_system.life_cycle_state,
                efs::LIFE_CYCLE_AVAILABLE
            )));
        }

        // Two of the four `Status` values are the statuses EFS REPORTS while it is
        // turning automatic backups on or off ("EFS is turning on automatic backups for
        // the file system"), so they are not statuses a caller can ask a policy to be
        // set to: a request names the backup state it wants the file system to end in.
        let requested = input.backup_policy.status.as_str();
        if requested != efs::BACKUP_ENABLED && requested != efs::BACKUP_DISABLED {
            return Err(bad_request(format!(
                "BackupPolicy.Status {requested} is a status Amazon EFS reports while it \
                 turns automatic backups on or off; specify {} or {}",
                efs::BACKUP_ENABLED,
                efs::BACKUP_DISABLED
            )));
        }

        let accepted =
            efs::BackupPolicyRecord::requested(file_system.backup_policy.as_ref(), requested, now);
        let described = accepted.describe();
        let Some(file_system) = state.file_systems.get_mut(&file_system_id) else {
            return Err(internal_server_error());
        };
        file_system.backup_policy = Some(accepted);

        Ok(PutBackupPolicyOutput {
            backup_policy: Some(described),
        })
    }
}

/// A 400 `BadRequest`: "the request is malformed or contains an error such as an invalid
/// parameter value or a missing required parameter".
fn bad_request(message: impl Into<String>) -> PutBackupPolicyError {
    PutBackupPolicyErrorModeled::BadRequest(BadRequest {
        error_code: "BadRequest".to_string(),
        message: Some(message.into()),
    })
    .into()
}

/// A 404 `FileSystemNotFound`: "the specified FileSystemId value doesn't exist in the
/// requester's Amazon Web Services account".
fn not_found(message: impl Into<String>) -> PutBackupPolicyError {
    PutBackupPolicyErrorModeled::FileSystemNotFound(FileSystemNotFound {
        error_code: "FileSystemNotFound".to_string(),
        message: Some(message.into()),
    })
    .into()
}

/// A 409 `IncorrectFileSystemLifeCycleState`: "the file system's lifecycle state is not
/// available".
fn incorrect_life_cycle_state(message: impl Into<String>) -> PutBackupPolicyError {
    PutBackupPolicyErrorModeled::IncorrectFileSystemLifeCycleState(
        IncorrectFileSystemLifeCycleState {
            error_code: "IncorrectFileSystemLifeCycleState".to_string(),
            message: Some(message.into()),
        },
    )
    .into()
}

/// A 500 `InternalServerError`, for the one case this handler cannot serve: the engine
/// this service was mounted on holds no store for it.
fn internal_server_error() -> PutBackupPolicyError {
    PutBackupPolicyErrorModeled::InternalServerError(InternalServerError {
        error_code: "InternalServerError".to_string(),
        message: Some("the EFS emulator state store is unavailable".to_string()),
    })
    .into()
}
