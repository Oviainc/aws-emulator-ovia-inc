// SPDX-License-Identifier: CC0-1.0

//! @op DeleteNotificationRule
//!
//! Removes the rule from the store — its name frees up for reuse and every target it
//! held loses this rule's subscription — and answers with the ARN of the rule that is
//! now gone. The operation models NO not-found error, so deleting an ARN the store does
//! not hold is a success that deletes nothing (an idempotent delete).
//!
//! ---
//!
//! # Modeled contract for `DeleteNotificationRule`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json")
//! ```
//!
//! Deletes a notification rule for a resource.
//!
//! ## Input
//!
//! * `Arn` (NotificationRuleArn, pattern
//!   ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$)
//!   **required** — The Amazon Resource Name (ARN) of the notification rule you want to
//!   delete.
//!
//! ## Output
//!
//! * `Arn` (NotificationRuleArn, pattern
//!   ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$) — The
//!   Amazon Resource Name (ARN) of the deleted notification rule.
//!
//! ## Errors
//!
//! * `ConcurrentModificationException` (HTTP 400) — CodeStar Notifications can't complete the
//!   request because the resource is being modified by another process. Wait a few minutes
//!   and try again.
//! * `LimitExceededException` (HTTP 400) — One of the CodeStar Notifications limits has been
//!   exceeded. Limits apply to accounts, notification rules, notifications, resources, and
//!   targets. For more information, see Limits.
//! * `ValidationException` (HTTP 400) — One or more parameter values are not valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteNotificationRuleError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteNotificationRuleError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `AccessDeniedException` is widened in through `extra_errors`: this SERVICE models the
// shape (`@httpError 403`, "you do not have sufficient permissions") and it is the answer
// the resource-based authorization check gives an `Arn` that names no notification rule,
// which every operation addressing a rule by ARN applies before it looks one up.
#[smithy_macros::operation_error(
    "com.amazonaws.codestarnotifications#DeleteNotificationRule",
    extra_errors = ["AccessDeniedException"]
)]
#[derive(Debug)]
pub enum DeleteNotificationRuleErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConcurrentModificationException(ConcurrentModificationException),
    LimitExceededException(LimitExceededException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_notification_rule::DeleteNotificationRuleError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteNotificationRuleError = crate::scaffolded::Scaffolded<DeleteNotificationRuleErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteNotificationRuleErrorModeled> for DeleteNotificationRuleError {
    fn from(modeled: DeleteNotificationRuleErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// HTTP 403 `AccessDeniedException`: the caller is not authorized for the resource the
/// request named, stated in this service's one text for the class.
fn not_authorized(arn: &str) -> DeleteNotificationRuleError {
    DeleteNotificationRuleErrorModeled::AccessDeniedException(AccessDeniedException {
        message: Some(crate::state::not_authorized_for(arn)),
    })
    .into()
}

/// The `DeleteNotificationRule` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteNotificationRule<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteNotificationRule<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteNotificationRule<Eng>
{
    type Input = DeleteNotificationRuleInput;
    type Output = DeleteNotificationRuleOutput;
    type Error = DeleteNotificationRuleError;

    const NAME: &'static str = "DeleteNotificationRule";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_NOTIFICATION_RULE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_NOTIFICATION_RULE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state as st;

        let region = st::request_region(ctx.parts());

        // Resource authorization comes FIRST, before any lookup: an `Arn` outside the
        // notification-rule form names no resource of this service, so nothing can
        // authorize the caller for it — and an idempotent delete must not report one as
        // deleted either.
        if !st::is_notification_rule_arn(&input.arn) {
            return Err(not_authorized(&input.arn));
        }

        let Some(state) = st::service_state(&*self.engine) else {
            // This service's store is the shared `InMemoryEngine`. Mounted on any other
            // engine there is no notification-rule store to delete from, which is a
            // "not written for this engine" condition, not a client error.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "DeleteNotificationRule (no state store on this engine)",
            ));
        };
        let mut store = state.lock();

        // `remove_rule` drops everything that only existed for this rule: its name
        // index entry, its target subscriptions and the tokens that served it (no token
        // may replay a rule that is gone). `None` means this region never held it,
        // which this operation models no error for.
        store.remove_rule(&region, &input.arn);

        Ok(DeleteNotificationRuleOutput {
            arn: Some(input.arn),
        })
    }
}
