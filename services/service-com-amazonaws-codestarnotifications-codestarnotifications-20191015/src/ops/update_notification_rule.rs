// SPDX-License-Identifier: CC0-1.0

//! @op UpdateNotificationRule
//!
//! Mutates only the members the request supplies on the stored rule (an absent member
//! leaves the stored value alone, a supplied list REPLACES the stored one), bumps
//! `LastModifiedTimestamp`, and answers with an empty body. Replacing `Targets`
//! unsubscribes the rule from the addresses it no longer carries and registers the ones
//! it gains.
//!
//! ---
//!
//! # Modeled contract for `UpdateNotificationRule`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json")
//! ```
//!
//! Updates a notification rule for a resource. You can change the events that trigger the
//! notification rule, the status of the rule, and the targets that receive the
//! notifications. To add or remove tags for a notification rule, you must use TagResource
//! and UntagResource.
//!
//! ## Input
//!
//! * `Arn` (NotificationRuleArn, pattern
//!   ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$)
//!   **required** — The Amazon Resource Name (ARN) of the notification rule.
//! * `DetailType` (DetailType, one of: BASIC | FULL) — The level of detail to include in the
//!   notifications for this resource. BASIC will include only the contents of the event as it
//!   would appear in Amazon CloudWatch. FULL will include any supplemental information
//!   provided by CodeStar Notifications and/or the service for the resource for which the
//!   notification is created.
//! * `EventTypeIds` (EventTypeIds) — A list of event types associated with this notification
//!   rule. For a complete list of event types and IDs, see Notification concepts in the
//!   Developer Tools Console User Guide.
//! * `Name` (NotificationRuleName, len 1..64, pattern ^[A-Za-z0-9\-_ ]+$) — The name of the
//!   notification rule.
//! * `Status` (NotificationRuleStatus, one of: DISABLED | ENABLED) — The status of the
//!   notification rule. Valid statuses include enabled (sending notifications) or disabled
//!   (not sending notifications).
//! * `Targets` (Targets, len 0..10) — The address and type of the targets to receive
//!   notifications from this notification rule.
//!
//! ## Errors
//!
//! * `ConfigurationException` (HTTP 400) — Some or all of the configuration is incomplete,
//!   missing, or not valid.
//! * `ResourceNotFoundException` (HTTP 404) — CodeStar Notifications can't find a resource
//!   that matches the provided ARN.
//! * `ValidationException` (HTTP 400) — One or more parameter values are not valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateNotificationRuleError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateNotificationRuleError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `AccessDeniedException` is widened in through `extra_errors`: this SERVICE models the
// shape (`@httpError 403`, "you do not have sufficient permissions") and it is the answer
// the resource-based authorization check gives an `Arn` that names no notification rule,
// which every operation addressing a rule by ARN applies before it looks one up.
#[smithy_macros::operation_error(
    "com.amazonaws.codestarnotifications#UpdateNotificationRule",
    extra_errors = ["AccessDeniedException"]
)]
#[derive(Debug)]
pub enum UpdateNotificationRuleErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConfigurationException(ConfigurationException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_notification_rule::UpdateNotificationRuleError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateNotificationRuleError = crate::scaffolded::Scaffolded<UpdateNotificationRuleErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateNotificationRuleErrorModeled> for UpdateNotificationRuleError {
    fn from(modeled: UpdateNotificationRuleErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// HTTP 403 `AccessDeniedException`: the caller is not authorized for the resource the
/// request named, stated in this service's one text for the class.
fn not_authorized(arn: &str) -> UpdateNotificationRuleError {
    UpdateNotificationRuleErrorModeled::AccessDeniedException(AccessDeniedException {
        message: Some(crate::state::not_authorized_for(arn)),
    })
    .into()
}

/// HTTP 400 `ValidationException`: "One or more parameter values are not valid."
fn invalid(message: impl Into<String>) -> UpdateNotificationRuleError {
    UpdateNotificationRuleErrorModeled::ValidationException(ValidationException {
        message: Some(message.into()),
    })
    .into()
}

/// HTTP 400 `ConfigurationException`: "Some or all of the configuration is incomplete,
/// missing, or not valid."
fn misconfigured(message: impl Into<String>) -> UpdateNotificationRuleError {
    UpdateNotificationRuleErrorModeled::ConfigurationException(ConfigurationException {
        message: Some(message.into()),
    })
    .into()
}

/// The `UpdateNotificationRule` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateNotificationRule<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateNotificationRule<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateNotificationRule<Eng>
{
    type Input = UpdateNotificationRuleInput;
    type Output = UpdateNotificationRuleOutput;
    type Error = UpdateNotificationRuleError;

    const NAME: &'static str = "UpdateNotificationRule";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_NOTIFICATION_RULE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_NOTIFICATION_RULE_OUTPUT_SCHEMA;
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

        if let Some(event_type_ids) = input.event_type_ids.as_deref() {
            // The same catalog the create path resolves against: a rule cannot be
            // re-pointed at an event the service does not publish either.
            if let Some(reason) = st::event_type_ids_violation(event_type_ids) {
                return Err(invalid(reason));
            }
        }
        if let Some(detail_type) = input.detail_type.as_deref() {
            if let Some(reason) = st::enum_violation("DetailType", detail_type, DETAIL_TYPE_VALUES)
            {
                return Err(invalid(reason));
            }
        }
        if let Some(status) = input.status.as_deref() {
            if let Some(reason) =
                st::enum_violation("Status", status, NOTIFICATION_RULE_STATUS_VALUES)
            {
                return Err(invalid(reason));
            }
        }
        let new_targets = match input.targets.as_deref() {
            Some(targets) => Some(st::rule_targets_from(targets).map_err(misconfigured)?),
            None => None,
        };

        let Some(state) = st::service_state(&*self.engine) else {
            // This service's store is the shared `InMemoryEngine`. Mounted on any other
            // engine there is no notification-rule store to update, which is a "not
            // written for this engine" condition, not a client error.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "UpdateNotificationRule (no state store on this engine)",
            ));
        };
        let mut store = state.lock();

        let Some(current) = store.rule_in_region(&region, &input.arn).cloned() else {
            // This operation names the missing rule by the ID its ARN carries.
            return Err(UpdateNotificationRuleErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(st::no_rule_with_identifier(st::rule_id_of_arn(&input.arn))),
                },
            )
            .into());
        };

        // Rule names stay unique per account and region, so a rename onto another
        // rule's name is refused rather than silently shadowing it.
        if let Some(name) = input.name.as_deref() {
            if name != current.name
                && store
                    .rule_names
                    .contains_key(&st::rule_name_key(&region, name))
            {
                // The name-uniqueness refusal states the class's one text and nothing of
                // the request: `Name` is `@sensitive`, and the rule that holds the name
                // is a resource this caller did not name. Only the SHAPE differs from the
                // create path, which models a conflict error this operation does not.
                return Err(invalid(st::RULE_NAME_ALREADY_IN_USE));
            }
        }

        let removed: Vec<String> = match &new_targets {
            Some(targets) => current
                .targets
                .iter()
                .filter(|existing| {
                    !targets
                        .iter()
                        .any(|target| target.target_address == existing.target_address)
                })
                .map(|existing| existing.target_address.clone())
                .collect(),
            None => Vec::new(),
        };

        let arn = current.arn.clone();
        let previous_name = current.name.clone();
        {
            let Some(rule) = store.rules.get_mut(&arn) else {
                return Err(UpdateNotificationRuleErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(st::no_rule_with_identifier(st::rule_id_of_arn(&arn))),
                    },
                )
                .into());
            };
            if let Some(name) = input.name.clone() {
                rule.name = name;
            }
            if let Some(status) = input.status.clone() {
                rule.status = status;
            }
            if let Some(detail_type) = input.detail_type.clone() {
                rule.detail_type = detail_type;
            }
            if let Some(event_type_ids) = input.event_type_ids.clone() {
                rule.event_type_ids = event_type_ids;
            }
            if let Some(targets) = new_targets.clone() {
                rule.targets = targets;
            }
            rule.last_modified_timestamp = st::now_timestamp();
        }

        if let Some(name) = input.name.clone() {
            if name != previous_name {
                store
                    .rule_names
                    .remove(&st::rule_name_key(&region, &previous_name));
                store
                    .rule_names
                    .insert(st::rule_name_key(&region, &name), arn.clone());
            }
        }
        for address in removed {
            store.unregister_target(&arn, &address);
        }
        for target in new_targets.into_iter().flatten() {
            store.register_target(&arn, &target);
        }

        Ok(UpdateNotificationRuleOutput {})
    }
}
