// SPDX-License-Identifier: CC0-1.0

//! @op PutFileSystemPolicy
//!
//! Applies a `FileSystemPolicy` — an IAM resource-based policy — to one file system, and
//! carries the POLICY LOCKOUT SAFETY CHECK: a document that would leave the calling
//! principal unable to make a future `PutFileSystemPolicy` request on this file system is
//! refused unless the request sets `BypassPolicyLockoutSafetyCheck`, the member's own
//! documented default being false. A refused request stores nothing, so the file system
//! goes on carrying the policy (or the absence of one) the refusal found.
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
//! # Modeled contract for `PutFileSystemPolicy`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-efs-magnolioapiservice-v20150201/smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json")
//! ```
//!
//! Applies an Amazon EFS FileSystemPolicy to an Amazon EFS file system. A file system
//! policy is an IAM resource-based policy and can contain multiple policy statements. A
//! file system always has exactly one file system policy, which can be the default policy
//! or an explicit policy set or updated using this API operation. EFS file system policies
//! have a 20,000 character limit. When an explicit policy is set, it overrides the default
//! policy. For more information about the default file system policy, see Default EFS file
//! system policy. EFS file system policies have a 20,000 character limit. This operation
//! requires permissions for the elasticfilesystem:PutFileSystemPolicy action.
//!
//! ## Input
//!
//! * `BypassPolicyLockoutSafetyCheck` (BypassPolicyLockoutSafetyCheck, default false)
//!   **required** — (Optional) A boolean that specifies whether or not to bypass the
//!   FileSystemPolicy lockout safety check. The lockout safety check determines whether the
//!   policy in the request will lock out, or prevent, the IAM principal that is making the
//!   request from making future PutFileSystemPolicy requests on this file system. Set
//!   BypassPolicyLockoutSafetyCheck to True only when you intend to prevent the IAM principal
//!   that is making the request from making subsequent PutFileSystemPolicy requests on this
//!   file system. The default value is False.
//! * `FileSystemId` (FileSystemId, len 0..128, pattern
//!   ^(arn:aws[-a-z]*:elasticfilesystem:[0-9a-z-:]+:file-system/fs-[0-9a-f]{8,40}|fs-[0-9a-f]{8,40})$)
//!   **required** — The ID of the EFS file system that you want to create or update the
//!   FileSystemPolicy for.
//! * `Policy` (Policy, len 1..20000, pattern ^[\s\S]+$) **required** — The FileSystemPolicy
//!   that you're creating. Accepts a JSON formatted policy definition. EFS file system
//!   policies have a 20,000 character limit. To find out more about the elements that make up
//!   a file system policy, see Resource-based policies within Amazon EFS.
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
//! * `IncorrectFileSystemLifeCycleState` (HTTP 409) — Returned if the file system's lifecycle
//!   state is not "available".
//! * `InternalServerError` (HTTP 500) — Returned if an error occurred on the server side.
//! * `InvalidPolicyException` (HTTP 400) — Returned if the FileSystemPolicy is malformed or
//!   contains an error such as a parameter value that is not valid or a missing required
//!   parameter. Returned in the case of a policy lockout safety check error.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `PutFileSystemPolicyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PutFileSystemPolicyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.efs#PutFileSystemPolicy")]
#[derive(Debug)]
pub enum PutFileSystemPolicyErrorModeled {
    BadRequest(BadRequest),
    FileSystemNotFound(FileSystemNotFound),
    IncorrectFileSystemLifeCycleState(IncorrectFileSystemLifeCycleState),
    InternalServerError(InternalServerError),
    InvalidPolicyException(InvalidPolicyException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_file_system_policy::PutFileSystemPolicyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutFileSystemPolicyError = crate::scaffolded::Scaffolded<PutFileSystemPolicyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutFileSystemPolicyErrorModeled> for PutFileSystemPolicyError {
    fn from(modeled: PutFileSystemPolicyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutFileSystemPolicy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutFileSystemPolicy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutFileSystemPolicy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutFileSystemPolicy<Eng>
{
    type Input = PutFileSystemPolicyInput;
    type Output = PutFileSystemPolicyOutput;
    type Error = PutFileSystemPolicyError;

    const NAME: &'static str = "PutFileSystemPolicy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_FILE_SYSTEM_POLICY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_FILE_SYSTEM_POLICY_OUTPUT_SCHEMA;
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

        // "The ID of the EFS file system that you want to create or update the
        // FileSystemPolicy for": the member accepts the id or the ARN of the file system.
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
        let file_system_arn = file_system.file_system_arn.clone();

        // "A file system policy is an IAM resource-based policy", and `Policy` "accepts a
        // JSON formatted policy definition" — the member's own constraints bound its
        // length and its characters, leaving the IAM JSON policy grammar to be judged
        // here: a body outside it is "malformed", and storing it would leave the file
        // system carrying a document no policy evaluation could read.
        if let Some(defect) = ::service_common::iam_policy::resource_policy_defect(&input.policy) {
            return Err(invalid_policy(format!(
                "The FileSystemPolicy is not a valid IAM resource-based policy document: \
                 {defect}."
            )));
        }

        // THE POLICY LOCKOUT SAFETY CHECK. "The lockout safety check determines whether
        // the policy in the request will lock out, or prevent, the IAM principal that is
        // making the request from making future PutFileSystemPolicy requests on this file
        // system", and is skipped exactly when the request asks for it to be
        // (`BypassPolicyLockoutSafetyCheck`, "the default value is False").
        if !input.bypass_policy_lockout_safety_check
            && locks_out_policy_updates(&input.policy, &file_system_arn)
        {
            return Err(invalid_policy(POLICY_LOCKOUT_MESSAGE));
        }

        // "A file system always has exactly one file system policy", so an accepted
        // document REPLACES whatever the file system carried. What is stored is the
        // document's canonical rendering rather than the caller's bytes, which is the one
        // layout every AWS API that takes a policy reports it in, so two callers who
        // submit the same policy written differently read the same text back.
        let stored = ::service_common::iam_policy::canonical_document(&input.policy);
        let Some(file_system) = state.file_systems.get_mut(&file_system_id) else {
            return Err(internal_server_error());
        };
        file_system.file_system_policy = Some(stored.clone());

        Ok(PutFileSystemPolicyOutput {
            file_system_id: Some(file_system_id),
            policy: Some(stored),
        })
    }
}

/// The IAM action that REPLACES a file system's policy: this service's ARN namespace
/// (`awsService$arnNamespace` = `elasticfilesystem`) and the operation name, which is how
/// every AWS API action is named. "This operation requires permissions for the
/// elasticfilesystem:PutFileSystemPolicy action."
const POLICY_MANAGEMENT_ACTION: &str = "elasticfilesystem:PutFileSystemPolicy";

/// The sentence the policy-lockout safety check refuses a request with. It is FIXED text:
/// it interpolates nothing from the request or from state (no file-system id, no
/// statement, no principal), names the member that bypasses the check, and is terminated
/// with a period.
const POLICY_LOCKOUT_MESSAGE: &str =
    "This policy would prevent future FileSystemPolicy updates. Retry the request with the \
     BypassPolicyLockoutSafetyCheck parameter to apply this policy.";

/// Whether `document`, applied to the file system `file_system_arn`, would leave the
/// calling principal unable to make a future [`POLICY_MANAGEMENT_ACTION`] request on it.
///
/// The question is the one IAM answers for any request — asked of the CANDIDATE document,
/// for the one action that can replace it — so it is decided by the tree's single
/// evaluation of the policy language rather than by a reading of its own. A file system
/// policy is not the only authority over that action ("a file system always has exactly
/// one file system policy, which can be the default policy", and an IAM principal's own
/// identity-based policies grant it too), so a document that simply stays SILENT about
/// the action leaves the path open: only an explicit `Deny` reaching the caller closes
/// it, which is IAM's own order — an explicit deny overrides every allow.
///
/// The caller is judged under every spelling a resource policy may name it by: its own
/// principal ARN, and the account-wide spellings a `Deny` over its account would refuse
/// it through.
fn locks_out_policy_updates(document: &str, file_system_arn: &str) -> bool {
    use ::service_common::iam_authorization as iam;

    let caller = ::service_common::caller_principal::ambient_user_arn();
    let mut principals = vec![caller.clone()];
    principals.extend(iam::account_principals(&caller));
    let request = iam::Request::new(POLICY_MANAGEMENT_ACTION, file_system_arn, &principals);
    iam::resource_decision(document, &request) == iam::Decision::Denied
}

/// A 400 `InvalidPolicyException`: "Returned if the FileSystemPolicy is malformed or
/// contains an error such as a parameter value that is not valid or a missing required
/// parameter. Returned in the case of a policy lockout safety check error."
fn invalid_policy(message: impl Into<String>) -> PutFileSystemPolicyError {
    PutFileSystemPolicyErrorModeled::InvalidPolicyException(InvalidPolicyException {
        error_code: Some("InvalidPolicyException".to_string()),
        message: Some(message.into()),
    })
    .into()
}

/// A 404 `FileSystemNotFound`: "the specified FileSystemId value doesn't exist in the
/// requester's Amazon Web Services account".
fn not_found(message: impl Into<String>) -> PutFileSystemPolicyError {
    PutFileSystemPolicyErrorModeled::FileSystemNotFound(FileSystemNotFound {
        error_code: "FileSystemNotFound".to_string(),
        message: Some(message.into()),
    })
    .into()
}

/// A 409 `IncorrectFileSystemLifeCycleState`: "the file system's lifecycle state is not
/// available".
fn incorrect_life_cycle_state(message: impl Into<String>) -> PutFileSystemPolicyError {
    PutFileSystemPolicyErrorModeled::IncorrectFileSystemLifeCycleState(
        IncorrectFileSystemLifeCycleState {
            error_code: "IncorrectFileSystemLifeCycleState".to_string(),
            message: Some(message.into()),
        },
    )
    .into()
}

/// A 500 `InternalServerError`, for the one case this handler cannot serve: the engine
/// this service was mounted on holds no store for it.
fn internal_server_error() -> PutFileSystemPolicyError {
    PutFileSystemPolicyErrorModeled::InternalServerError(InternalServerError {
        error_code: "InternalServerError".to_string(),
        message: Some("the EFS emulator state store is unavailable".to_string()),
    })
    .into()
}
