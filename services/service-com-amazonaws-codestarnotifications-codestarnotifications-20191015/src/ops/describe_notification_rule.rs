// SPDX-License-Identifier: CC0-1.0

//! @op DescribeNotificationRule
//!
//! Reports the PERSISTED rule [`crate::state`] holds for an ARN: its name, resource,
//! detail type, status, tags, creator and timestamps, its stored event type ids
//! resolved through the event-type catalog, and its targets with the account-wide
//! status the target registry holds for each address.
//!
//! This is a reporting operation: it writes nothing.
//!
//! ---
//!
//! # Modeled contract for `DescribeNotificationRule`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json")
//! ```
//!
//! Returns information about a specified notification rule.
//!
//! ## Input
//!
//! * `Arn` (NotificationRuleArn, pattern
//!   ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$)
//!   **required** — The Amazon Resource Name (ARN) of the notification rule.
//!
//! ## Output
//!
//! * `Arn` (NotificationRuleArn, pattern
//!   ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$)
//!   **required** — The Amazon Resource Name (ARN) of the notification rule.
//! * `CreatedBy` (NotificationRuleCreatedBy, len 1..) — The name or email alias of the person
//!   who created the notification rule.
//! * `CreatedTimestamp` (CreatedTimestamp) — The date and time the notification rule was
//!   created, in timestamp format.
//! * `DetailType` (DetailType, one of: BASIC | FULL) — The level of detail included in the
//!   notifications for this resource. BASIC will include only the contents of the event as it
//!   would appear in Amazon CloudWatch. FULL will include any supplemental information
//!   provided by CodeStar Notifications and/or the service for the resource for which the
//!   notification is created.
//! * `EventTypes` (EventTypeBatch) — A list of the event types associated with the
//!   notification rule.
//! * `LastModifiedTimestamp` (LastModifiedTimestamp) — The date and time the notification
//!   rule was most recently updated, in timestamp format.
//! * `Name` (NotificationRuleName, len 1..64, pattern ^[A-Za-z0-9\-_ ]+$) — The name of the
//!   notification rule.
//! * `Resource` (NotificationRuleResource, pattern
//!   ^arn:aws[^:\s]*:[^:\s]*:[^:\s]*:[0-9]{12}:[^\s]+$) — The Amazon Resource Name (ARN) of
//!   the resource associated with the notification rule.
//! * `Status` (NotificationRuleStatus, one of: DISABLED | ENABLED) — The status of the
//!   notification rule. Valid statuses are on (sending notifications) or off (not sending
//!   notifications).
//! * `Tags` (Tags) — The tags associated with the notification rule.
//! * `Targets` (TargetsBatch) — A list of the Amazon Q Developer in chat applications topics
//!   and Amazon Q Developer in chat applications clients associated with the notification
//!   rule.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 404) — CodeStar Notifications can't find a resource
//!   that matches the provided ARN.
//! * `ValidationException` (HTTP 400) — One or more parameter values are not valid.
//! * `AccessDeniedException` (HTTP 403) — widened in through `extra_errors` (see below):
//!   the answer the resource-based authorization check gives an `Arn` that names no
//!   notification rule of this service.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeNotificationRuleError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeNotificationRuleError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `AccessDeniedException` is widened in through `extra_errors`: this SERVICE models the
// shape (`@httpError 403`, "you do not have sufficient permissions") and it is the answer
// the resource-based authorization check gives an `Arn` that names no notification rule,
// which this operation applies before any lookup — deliberately distinct from the 404 it
// answers for a well-formed ARN no rule holds.
#[smithy_macros::operation_error(
    "com.amazonaws.codestarnotifications#DescribeNotificationRule",
    extra_errors = ["AccessDeniedException"]
)]
#[derive(Debug)]
pub enum DescribeNotificationRuleErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_notification_rule::DescribeNotificationRuleError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeNotificationRuleError = crate::scaffolded::Scaffolded<DescribeNotificationRuleErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeNotificationRuleErrorModeled> for DescribeNotificationRuleError {
    fn from(modeled: DescribeNotificationRuleErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// HTTP 403 `AccessDeniedException`: the caller is not authorized for the resource the
/// request named, stated in this service's one text for the class.
fn not_authorized(arn: &str) -> DescribeNotificationRuleError {
    DescribeNotificationRuleErrorModeled::AccessDeniedException(AccessDeniedException {
        message: Some(crate::state::not_authorized_for(arn)),
    })
    .into()
}

/// The `DescribeNotificationRule` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeNotificationRule<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeNotificationRule<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeNotificationRule<Eng>
{
    type Input = DescribeNotificationRuleInput;
    type Output = DescribeNotificationRuleOutput;
    type Error = DescribeNotificationRuleError;

    const NAME: &'static str = "DescribeNotificationRule";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_NOTIFICATION_RULE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_NOTIFICATION_RULE_OUTPUT_SCHEMA;
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
        // authorize the caller for it. A well-formed ARN no rule holds is the 404 below.
        if !st::is_notification_rule_arn(&input.arn) {
            return Err(not_authorized(&input.arn));
        }

        let Some(state) = st::service_state(&*self.engine) else {
            // This service's store is the shared `InMemoryEngine`. Mounted on any other
            // engine there is no notification-rule store to report from, which is a
            // "not written for this engine" condition, not a client error.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "DescribeNotificationRule (no state store on this engine)",
            ));
        };
        let store = state.lock();

        let Some(rule) = store.rule_in_region(&region, &input.arn).cloned() else {
            return Err(
                DescribeNotificationRuleErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(st::no_rule_with_id(&input.arn)),
                    },
                )
                .into(),
            );
        };

        let event_types = rule
            .event_type_ids
            .iter()
            .map(|id| {
                let resolved = st::resolve_event_type(id);
                EventTypeSummary {
                    event_type_id: Some(resolved.event_type_id),
                    service_name: resolved.service_name,
                    event_type_name: resolved.event_type_name,
                    resource_type: resolved.resource_type,
                }
            })
            .collect();
        let targets = rule
            .targets
            .iter()
            .map(|target| TargetSummary {
                target_address: Some(target.target_address.clone()),
                target_type: Some(target.target_type.clone()),
                target_status: Some(store.target_status(&target.target_address)),
            })
            .collect();

        Ok(DescribeNotificationRuleOutput {
            arn: rule.arn,
            name: Some(rule.name),
            event_types: Some(event_types),
            resource: Some(rule.resource),
            targets: Some(targets),
            detail_type: Some(rule.detail_type),
            created_by: Some(rule.created_by),
            status: Some(rule.status),
            created_timestamp: Some(rule.created_timestamp),
            last_modified_timestamp: Some(rule.last_modified_timestamp),
            // `Tags` reports the rule's own tag map: a rule with no tags carries zero
            // entries, which is an EMPTY MAP on the wire, not an absent member.
            tags: Some(rule.tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        })
    }
}
