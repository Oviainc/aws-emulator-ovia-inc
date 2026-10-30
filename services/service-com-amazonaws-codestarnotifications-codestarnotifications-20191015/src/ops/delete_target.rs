// SPDX-License-Identifier: CC0-1.0

//! @op DeleteTarget
//!
//! Deletes one target of the account: the address loses its registration, so it leaves
//! `ListTargets`. A target exists exactly while some notification rule subscribes to its
//! address, so deleting it means dropping those subscriptions — which is what
//! `ForceUnsubscribeAll` authorises. Without that authorisation an address a rule still
//! subscribes to is NOT deleted: the call is refused with the one client error this
//! operation models, so no caller silently loses a rule's target list.
//!
//! Deleting an address no rule subscribes to is a success that deletes nothing (this
//! operation models no not-found error), which makes the delete idempotent.
//!
//! Like every other operation of this service, it is served by a regional endpoint: the
//! rules it unsubscribes are those of the region the request was signed for, so a rule of
//! another region keeps its own subscription — and with it the registry entry.
//!
//! ---
//!
//! # Modeled contract for `DeleteTarget`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json")
//! ```
//!
//! Deletes a specified target for notifications.
//!
//! ## Input
//!
//! * `ForceUnsubscribeAll` (ForceUnsubscribeAll, default false) **required** — A Boolean
//!   value that can be used to delete all associations with this Amazon Q Developer in chat
//!   applications topic. The default value is FALSE. If set to TRUE, all associations between
//!   that target and every notification rule in your Amazon Web Services account are deleted.
//! * `TargetAddress` (TargetAddress, len 1..320) **required** — The Amazon Resource Name
//!   (ARN) of the Amazon Q Developer in chat applications topic or Amazon Q Developer in chat
//!   applications client to delete.
//!
//! ## Errors
//!
//! * `ValidationException` (HTTP 400) — One or more parameter values are not valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteTargetError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteTargetError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.codestarnotifications#DeleteTarget")]
#[derive(Debug)]
pub enum DeleteTargetErrorModeled {
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_target::DeleteTargetError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteTargetError = crate::scaffolded::Scaffolded<DeleteTargetErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteTargetErrorModeled> for DeleteTargetError {
    fn from(modeled: DeleteTargetErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// HTTP 400 `ValidationException`, the only client error this operation models: "One or
/// more parameter values are not valid."
fn invalid(message: impl Into<String>) -> DeleteTargetError {
    DeleteTargetErrorModeled::ValidationException(ValidationException {
        message: Some(message.into()),
    })
    .into()
}

/// How this operation states that the address is still in use and the request did not
/// authorise dropping those associations.
///
/// The text names the member that authorises them (`ForceUnsubscribeAll`, documented as
/// deleting "all associations between that target and every notification rule"), and
/// states how many rules hold the address, which is what a caller needs in order to
/// decide. It does NOT repeat the address: `TargetAddress` is `@sensitive`, so it stays
/// out of the response.
fn target_still_in_use(rules: usize) -> String {
    let noun = if rules == 1 {
        "notification rule"
    } else {
        "notification rules"
    };
    format!(
        "The target was not deleted because it is used by {rules} {noun}. Set \
         ForceUnsubscribeAll to true to delete the target and remove it from every \
         notification rule that uses it."
    )
}

/// The `DeleteTarget` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteTarget<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteTarget<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteTarget<Eng>
{
    type Input = DeleteTargetInput;
    type Output = DeleteTargetOutput;
    type Error = DeleteTargetError;

    const NAME: &'static str = "DeleteTarget";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_TARGET_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_TARGET_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state as st;

        let region = st::request_region(ctx.parts());

        let Some(state) = st::service_state(&*self.engine) else {
            // This service's store is the shared `InMemoryEngine`. Mounted on any other
            // engine there is no target registry to delete from, which is a "not written
            // for this engine" condition, not a client error.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "DeleteTarget (no state store on this engine)",
            ));
        };
        let mut store = state.lock();

        // Every rule of THIS region subscribing to the address, resolved once. The
        // region scoping lives in `state.rs`, beside the registry it indexes.
        let subscribed = store.rules_subscribed_to(&region, &input.target_address);

        if !subscribed.is_empty() && !input.force_unsubscribe_all {
            return Err(invalid(target_still_in_use(subscribed.len())));
        }

        // ONE instant for every rule this call touches: they all stopped carrying the
        // target at the same moment.
        let modified_at = st::now_timestamp();
        for arn in &subscribed {
            if let Some(rule) = store.rule_in_region_mut(&region, arn) {
                rule.targets
                    .retain(|target| target.target_address != input.target_address);
                rule.last_modified_timestamp = modified_at;
            }
            // The registry keeps the address only while some rule still subscribes to it,
            // so the registration goes with the last subscription dropped here.
            store.unregister_target(arn, &input.target_address);
        }

        Ok(DeleteTargetOutput {})
    }
}
