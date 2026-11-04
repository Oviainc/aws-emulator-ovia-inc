// SPDX-License-Identifier: CC0-1.0

//! @op DeleteApplicationStatusCheck
//!
//! Deletes the application status check the request names and answers with it as this call
//! leaves it: the stored configuration carrying "the date and time when the application
//! status check was deleted" (`ApplicationStatusCheckResponseObject$DeletionTime`), which is
//! what makes it one inside its deletion grace period — reported by
//! `DescribeApplicationStatusChecks` only to a caller that asks for deleted checks, and no
//! longer counted against the account's quota of checks.
//!
//! "Deleting a check automatically removes all of its associations", so both halves go: the
//! tag associations the check's own `TargetTagAssociations` member holds, and the per-check
//! results the associated instances held, which `DescribeApplicationStatus` stops
//! aggregating for them. The store's own writer
//! ([`crate::state::ComAmazonawsEc2Amazonec2State::begin_application_status_check_deletion`])
//! performs all of it, so this answer and what a later describe reports cannot disagree.
//!
//! A check already inside its deletion grace period has been deleted once; this call reports
//! it as that delete left it and deletes nothing a second time, which is what makes a
//! retried request — "if you retry a request with the same token, the service ignores the
//! request but does not return an error" (`$ClientToken`) — answer what the first one
//! answered. An id the store holds no check for at all is refused rather than answered: a
//! caller that deletes twice has to be able to tell the deletion it performed from one it
//! did not.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The store is
//! reached through [`crate::state::require_service_state`], the way every handler beside this
//! one reaches it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteApplicationStatusCheck`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Deletes an application status check. The following rules apply: Deleting a check
//! automatically removes all of its associations. Use DescribeApplicationStatusChecks to
//! view existing checks before deleting.
//!
//! ## Input
//!
//! * `ApplicationStatusCheckId` (ApplicationStatusCheckId) **required** — The ID of the
//!   application status check to delete.
//! * `ClientToken` (String, @idempotencyToken) — A unique, case-sensitive identifier that you
//!   provide to ensure that the operation completes no more than one time. If you retry a
//!   request with the same token, the service ignores the request but does not return an
//!   error. For more information, see Ensuring idempotency.
//! * `DryRun` (Boolean) — Checks whether you have the required permissions for the operation,
//!   without actually making the request, and provides an error response. If you have the
//!   required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//!
//! ## Output
//!
//! * `ApplicationStatusCheck` (ApplicationStatusCheckResponseObject) — Information about the
//!   deleted application status check.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteApplicationStatusCheckError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteApplicationStatusCheckError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#DeleteApplicationStatusCheck")]
#[derive(Debug)]
pub enum DeleteApplicationStatusCheckErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_application_status_check::DeleteApplicationStatusCheckError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteApplicationStatusCheckError = crate::scaffolded::Scaffolded<DeleteApplicationStatusCheckErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteApplicationStatusCheckErrorModeled> for DeleteApplicationStatusCheckError {
    fn from(modeled: DeleteApplicationStatusCheckErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteApplicationStatusCheck` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteApplicationStatusCheck<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteApplicationStatusCheck<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteApplicationStatusCheck<Eng>
{
    type Input = DeleteApplicationStatusCheckInput;
    type Output = DeleteApplicationStatusCheckOutput;
    type Error = DeleteApplicationStatusCheckError;

    const NAME: &'static str = "DeleteApplicationStatusCheck";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_APPLICATION_STATUS_CHECK_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_APPLICATION_STATUS_CHECK_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use ::std::time::SystemTime;

        use crate::scaffolded::Scaffolded;
        use crate::state::{
            require_known_application_status_check, require_service_state, MAX_CLIENT_TOKEN_CHARS,
        };

        // `ctx` carries request metadata only; this operation reads none of it — which check a
        // request deletes is a fact of the id it names, not of the Region it was signed for.
        // The store is reached through the engine held on `self`.
        let _ = ctx;

        let application_status_check_id = input.application_status_check_id.clone();

        let state = require_service_state(&*self.engine)?;
        let mut store = state.lock();

        // "The ID of the application status check to delete": the SUBJECT of the call is
        // resolved before anything else is read — a value that is no id of this kind first,
        // then a well-formed id the store holds no check for — the order the one reader this
        // family shares keeps, so a caller can tell those two from every other reason a
        // request is refused.
        require_known_application_status_check(&store, &application_status_check_id)?;

        // "A unique, case-sensitive identifier that you provide to ensure that the operation
        // completes no more than one time" (`$ClientToken`): this operation keeps no record of
        // the token — a check inside its deletion grace period is what answers a retry — but
        // the value a caller sends is still held to the length every token of this crate is.
        if input
            .client_token
            .as_ref()
            .is_some_and(|token| token.chars().count() > MAX_CLIENT_TOKEN_CHARS)
        {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "clientToken accepts at most {MAX_CLIENT_TOKEN_CHARS} characters"
            )));
        }

        // DryRun sits AFTER every parameter has been validated and BEFORE the first write:
        // the flag asks whether the request WOULD have succeeded, so one that would have been
        // refused is refused with that reason instead, and one that would have succeeded
        // leaves state untouched — "if you have the required permissions, the error response
        // is DryRunOperation" (`$DryRun`).
        if input.dry_run == Some(true) {
            return Err(Scaffolded::dry_run_operation());
        }

        // A check whose deletion has already happened is reported as THAT delete left it: its
        // deletion instant stands, its associations are already gone, and nothing is written a
        // second time, so a retried delete answers what the first one answered. That rule lives
        // in the store's writer, with the delete it is about.
        //
        // "Information about the deleted application status check"
        // (`DeleteApplicationStatusCheckResult$ApplicationStatusCheck`): the record as that
        // writer leaves it rather than one rebuilt here, so this answer is what a describe
        // asking for deleted checks reports.
        let deleted = store
            .begin_application_status_check_deletion(
                &application_status_check_id,
                SystemTime::now(),
            )
            .ok_or_else(|| {
                Scaffolded::internal_error(format!(
                    "the application status check '{application_status_check_id}' was resolved \
                     and then could not be deleted"
                ))
            })?;

        Ok(DeleteApplicationStatusCheckOutput {
            application_status_check: Some(deleted),
        })
    }
}
