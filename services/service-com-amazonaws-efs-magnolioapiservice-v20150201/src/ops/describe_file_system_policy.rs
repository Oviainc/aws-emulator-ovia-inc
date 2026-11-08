// SPDX-License-Identifier: CC0-1.0

//! @op DescribeFileSystemPolicy
//!
//! Reports the `FileSystemPolicy` one file system CARRIES — and refuses with
//! `PolicyNotFound` while it carries none, because a resource policy is a stored document
//! rather than a member every file system has.
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
//! # Modeled contract for `DescribeFileSystemPolicy`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-efs-magnolioapiservice-v20150201/smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json")
//! ```
//!
//! Returns the FileSystemPolicy for the specified EFS file system. This operation requires
//! permissions for the elasticfilesystem:DescribeFileSystemPolicy action.
//!
//! ## Input
//!
//! * `FileSystemId` (FileSystemId, len 0..128, pattern
//!   ^(arn:aws[-a-z]*:elasticfilesystem:[0-9a-z-:]+:file-system/fs-[0-9a-f]{8,40}|fs-[0-9a-f]{8,40})$)
//!   **required** — Specifies which EFS file system to retrieve the FileSystemPolicy for.
//!
//! ## Output
//!
//! * `FileSystemId` (FileSystemId, len 0..128, pattern
//!   ^(arn:aws[-a-z]*:elasticfilesystem:[0-9a-z-:]+:file-system/fs-[0-9a-f]{8,40}|fs-[0-9a-f]{8,40})$)
//!   — Specifies the EFS file system to which the FileSystemPolicy applies.
//! * `Policy` (Policy, len 1..20000, pattern ^[\s\S]+$) — The JSON formatted FileSystemPolicy
//!   for the EFS file system.
//!
//! ## Errors
//!
//! * `BadRequest` (HTTP 400) — Returned if the request is malformed or contains an error such
//!   as an invalid parameter value or a missing required parameter.
//! * `FileSystemNotFound` (HTTP 404) — Returned if the specified FileSystemId value doesn't
//!   exist in the requester's Amazon Web Services account.
//! * `InternalServerError` (HTTP 500) — Returned if an error occurred on the server side.
//! * `PolicyNotFound` (HTTP 404) — Returned if no backup is specified for a One Zone EFS file
//!   system.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeFileSystemPolicyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeFileSystemPolicyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.efs#DescribeFileSystemPolicy")]
#[derive(Debug)]
pub enum DescribeFileSystemPolicyErrorModeled {
    BadRequest(BadRequest),
    FileSystemNotFound(FileSystemNotFound),
    InternalServerError(InternalServerError),
    PolicyNotFound(PolicyNotFound),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_file_system_policy::DescribeFileSystemPolicyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeFileSystemPolicyError = crate::scaffolded::Scaffolded<DescribeFileSystemPolicyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeFileSystemPolicyErrorModeled> for DescribeFileSystemPolicyError {
    fn from(modeled: DescribeFileSystemPolicyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeFileSystemPolicy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeFileSystemPolicy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeFileSystemPolicy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeFileSystemPolicy<Eng>
{
    type Input = DescribeFileSystemPolicyInput;
    type Output = DescribeFileSystemPolicyOutput;
    type Error = DescribeFileSystemPolicyError;

    const NAME: &'static str = "DescribeFileSystemPolicy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_FILE_SYSTEM_POLICY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_FILE_SYSTEM_POLICY_OUTPUT_SCHEMA;
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
        // Reporting only: the sole write is a transition the clock already made due,
        // owed at this instant whether or not anybody read it.
        state.materialize_due_transitions(now);

        // "Specifies which EFS file system to retrieve the FileSystemPolicy for": the
        // member accepts the id or the ARN of the file system.
        let file_system_id = efs::file_system_id_of(&input.file_system_id).to_string();
        let Some(file_system) = state.file_systems.get(&file_system_id) else {
            return Err(not_found(efs::file_system_not_found_message(
                &file_system_id,
            )));
        };
        // A file system no policy was ever stored on HAS no policy — which is not the
        // same as an empty one — so the read reports the absence.
        let Some(policy) = file_system.file_system_policy.clone() else {
            return Err(
                DescribeFileSystemPolicyErrorModeled::PolicyNotFound(efs::policy_not_found())
                    .into(),
            );
        };

        Ok(DescribeFileSystemPolicyOutput {
            file_system_id: Some(file_system_id),
            policy: Some(policy),
        })
    }
}

/// A 404 `FileSystemNotFound`: "the specified FileSystemId value doesn't exist in the
/// requester's Amazon Web Services account".
fn not_found(message: impl Into<String>) -> DescribeFileSystemPolicyError {
    DescribeFileSystemPolicyErrorModeled::FileSystemNotFound(FileSystemNotFound {
        error_code: "FileSystemNotFound".to_string(),
        message: Some(message.into()),
    })
    .into()
}

/// A 500 `InternalServerError`, for the one case this handler cannot serve: the engine
/// this service was mounted on holds no store for it.
fn internal_server_error() -> DescribeFileSystemPolicyError {
    DescribeFileSystemPolicyErrorModeled::InternalServerError(InternalServerError {
        error_code: "InternalServerError".to_string(),
        message: Some("the EFS emulator state store is unavailable".to_string()),
    })
    .into()
}
