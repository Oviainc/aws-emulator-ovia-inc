// SPDX-License-Identifier: CC0-1.0

//! @op Unsubscribe
//!
//! Removes one target's association with a stored notification rule: the address is
//! detached from the rule's target list and its subscription dropped from the
//! account-wide target registry, so the next `DescribeNotificationRule` no longer
//! reports it. Returns the rule's ARN.
//!
//! Removal is idempotent: an address the rule does not carry leaves the rule unchanged
//! and still answers 200, because the association the caller asked to remove is already
//! absent.
//!
//! ---
//!
//! # Modeled contract for `Unsubscribe`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json")
//! ```
//!
//! Removes an association between a notification rule and an Amazon Q Developer in chat
//! applications topic so that subscribers to that topic stop receiving notifications when
//! the events described in the rule are triggered.
//!
//! ## Input
//!
//! * `Arn` (NotificationRuleArn, pattern
//!   ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$)
//!   **required** — The Amazon Resource Name (ARN) of the notification rule.
//! * `TargetAddress` (TargetAddress, len 1..320) **required** — The ARN of the Amazon Q
//!   Developer in chat applications topic to unsubscribe from the notification rule.
//!
//! ## Output
//!
//! * `Arn` (NotificationRuleArn, pattern
//!   ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$)
//!   **required** — The Amazon Resource Name (ARN) of the the notification rule from which
//!   you have removed a subscription.
//!
//! ## Errors
//!
//! * `ValidationException` (HTTP 400) — One or more parameter values are not valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UnsubscribeError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UnsubscribeError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `AccessDeniedException` is widened in through `extra_errors`: this SERVICE models the
// shape (`@httpError 403`, "you do not have sufficient permissions") and it is the answer
// the resource-based authorization check gives an `Arn` that names no notification rule,
// which every operation addressing a rule by ARN applies before it looks one up.
#[smithy_macros::operation_error(
    "com.amazonaws.codestarnotifications#Unsubscribe",
    extra_errors = ["AccessDeniedException"]
)]
#[derive(Debug)]
pub enum UnsubscribeErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::unsubscribe::UnsubscribeError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UnsubscribeError = crate::scaffolded::Scaffolded<UnsubscribeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UnsubscribeErrorModeled> for UnsubscribeError {
    fn from(modeled: UnsubscribeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// HTTP 400 `ValidationException`, the only client error this operation models: "One or
/// more parameter values are not valid."
fn invalid(message: impl Into<String>) -> UnsubscribeError {
    UnsubscribeErrorModeled::ValidationException(ValidationException {
        message: Some(message.into()),
    })
    .into()
}

/// HTTP 403 `AccessDeniedException`: the caller is not authorized for the resource the
/// request named, stated in this service's one text for the class.
fn not_authorized(arn: &str) -> UnsubscribeError {
    UnsubscribeErrorModeled::AccessDeniedException(AccessDeniedException {
        message: Some(crate::state::not_authorized_for(arn)),
    })
    .into()
}

/// The `Unsubscribe` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct Unsubscribe<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> Unsubscribe<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for Unsubscribe<Eng>
{
    type Input = UnsubscribeInput;
    type Output = UnsubscribeOutput;
    type Error = UnsubscribeError;

    const NAME: &'static str = "Unsubscribe";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UNSUBSCRIBE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UNSUBSCRIBE_OUTPUT_SCHEMA;
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
        // authorize the caller for it.
        if !st::is_notification_rule_arn(&input.arn) {
            return Err(not_authorized(&input.arn));
        }

        let Some(state) = st::service_state(&*self.engine) else {
            // This service's store is the shared `InMemoryEngine`. Mounted on any other
            // engine there is no notification-rule store to unsubscribe from, which is a
            // "not written for this engine" condition, not a client error.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "Unsubscribe (no state store on this engine)",
            ));
        };
        let mut store = state.lock();

        // ONE region-scoped lookup. This operation models no not-found error, so a rule
        // this region does not hold is refused with the one client error it does model —
        // stating the same absence the not-found paths state, named by the rule ID the
        // submitted ARN carries.
        let arn = input.arn.clone();
        let detached = {
            let Some(rule) = store.rule_in_region_mut(&region, &arn) else {
                return Err(invalid(st::no_rule_with_identifier(st::rule_id_of_arn(
                    &arn,
                ))));
            };
            let before = rule.targets.len();
            rule.targets
                .retain(|target| target.target_address != input.target_address);
            let detached = rule.targets.len() != before;
            if detached {
                rule.last_modified_timestamp = st::now_timestamp();
            }
            detached
        };
        if detached {
            // The account-wide registry keeps the address only while some rule still
            // subscribes to it.
            store.unregister_target(&arn, &input.target_address);
        }

        Ok(UnsubscribeOutput { arn })
    }
}
