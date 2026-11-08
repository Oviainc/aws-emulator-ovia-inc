// SPDX-License-Identifier: CC0-1.0

//! @op DeleteFileSystemPolicy
//!
//! Removes the `FileSystemPolicy` one file system carries, after which "the default
//! FileSystemPolicy goes into effect" — the state a file system none was ever stored on is
//! already in, so the delete is idempotent and models no not-found error for the policy
//! itself.
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
//! # Modeled contract for `DeleteFileSystemPolicy`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-efs-magnolioapiservice-v20150201/smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json")
//! ```
//!
//! Deletes the FileSystemPolicy for the specified file system. The default FileSystemPolicy
//! goes into effect once the existing policy is deleted. For more information about the
//! default file system policy, see Using Resource-based Policies with EFS. This operation
//! requires permissions for the elasticfilesystem:DeleteFileSystemPolicy action.
//!
//! ## Input
//!
//! * `FileSystemId` (FileSystemId, len 0..128, pattern
//!   ^(arn:aws[-a-z]*:elasticfilesystem:[0-9a-z-:]+:file-system/fs-[0-9a-f]{8,40}|fs-[0-9a-f]{8,40})$)
//!   **required** — Specifies the EFS file system for which to delete the FileSystemPolicy.
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

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteFileSystemPolicyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json");

#[derive(Debug, Clone, Default)]
pub struct DeleteFileSystemPolicyOutput;

impl DeleteFileSystemPolicyOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "DELETE",
        "/2015-02-01/file-systems/{FileSystemId}/policy",
        Some(200u16),
    ));
}

pub static DELETE_FILE_SYSTEM_POLICY_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &DeleteFileSystemPolicyOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DeleteFileSystemPolicyOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteFileSystemPolicyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.efs#DeleteFileSystemPolicy")]
#[derive(Debug)]
pub enum DeleteFileSystemPolicyErrorModeled {
    BadRequest(BadRequest),
    FileSystemNotFound(FileSystemNotFound),
    IncorrectFileSystemLifeCycleState(IncorrectFileSystemLifeCycleState),
    InternalServerError(InternalServerError),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_file_system_policy::DeleteFileSystemPolicyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteFileSystemPolicyError = crate::scaffolded::Scaffolded<DeleteFileSystemPolicyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteFileSystemPolicyErrorModeled> for DeleteFileSystemPolicyError {
    fn from(modeled: DeleteFileSystemPolicyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteFileSystemPolicy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteFileSystemPolicy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteFileSystemPolicy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteFileSystemPolicy<Eng>
{
    type Input = DeleteFileSystemPolicyInput;
    type Output = DeleteFileSystemPolicyOutput;
    type Error = DeleteFileSystemPolicyError;

    const NAME: &'static str = "DeleteFileSystemPolicy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_FILE_SYSTEM_POLICY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = DELETE_FILE_SYSTEM_POLICY_OUTPUT_SCHEMA;
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

        // "Specifies the EFS file system for which to delete the FileSystemPolicy": the
        // member accepts the id or the ARN of the file system.
        let file_system_id = efs::file_system_id_of(&input.file_system_id).to_string();
        let Some(file_system) = state.file_systems.get_mut(&file_system_id) else {
            return Err(not_found(efs::file_system_not_found_message(
                &file_system_id,
            )));
        };
        // `IncorrectFileSystemLifeCycleState`: "Returned if the file system's lifecycle
        // state is not available."
        if file_system.life_cycle_state != efs::LIFE_CYCLE_AVAILABLE {
            let life_cycle_state = file_system.life_cycle_state.clone();
            return Err(incorrect_life_cycle_state(format!(
                "File system {} is in life cycle state {life_cycle_state}, not {}",
                efs::quoted(&file_system_id),
                efs::LIFE_CYCLE_AVAILABLE
            )));
        }
        // The operation models no "no policy" error: what it leaves behind is a file
        // system under "the default FileSystemPolicy", which is exactly where one that
        // never carried an explicit policy already is — so deleting twice is deleting
        // once, and the policy read reports the absence either way.
        file_system.file_system_policy = None;

        Ok(DeleteFileSystemPolicyOutput)
    }
}

/// A 404 `FileSystemNotFound`: "the specified FileSystemId value doesn't exist in the
/// requester's Amazon Web Services account".
fn not_found(message: impl Into<String>) -> DeleteFileSystemPolicyError {
    DeleteFileSystemPolicyErrorModeled::FileSystemNotFound(FileSystemNotFound {
        error_code: "FileSystemNotFound".to_string(),
        message: Some(message.into()),
    })
    .into()
}

/// A 409 `IncorrectFileSystemLifeCycleState`: "the file system's lifecycle state is not
/// available".
fn incorrect_life_cycle_state(message: impl Into<String>) -> DeleteFileSystemPolicyError {
    DeleteFileSystemPolicyErrorModeled::IncorrectFileSystemLifeCycleState(
        IncorrectFileSystemLifeCycleState {
            error_code: "IncorrectFileSystemLifeCycleState".to_string(),
            message: Some(message.into()),
        },
    )
    .into()
}

/// A 500 `InternalServerError`, for the one case this handler cannot serve: the engine
/// this service was mounted on holds no store for it.
fn internal_server_error() -> DeleteFileSystemPolicyError {
    DeleteFileSystemPolicyErrorModeled::InternalServerError(InternalServerError {
        error_code: "InternalServerError".to_string(),
        message: Some("the EFS emulator state store is unavailable".to_string()),
    })
    .into()
}
