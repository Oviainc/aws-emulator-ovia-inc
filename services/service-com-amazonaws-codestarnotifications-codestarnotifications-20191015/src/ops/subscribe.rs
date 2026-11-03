// SPDX-License-Identifier: CC0-1.0

//! @op Subscribe
//!
//! Associates one target with a stored notification rule: the target is upserted on the
//! rule (one subscription per `TargetAddress`) and registered in the account-wide
//! target registry, so the next `DescribeNotificationRule` and `ListTargets` both see
//! it. Returns the rule's ARN.
//!
//! ---
//!
//! # Modeled contract for `Subscribe`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json")
//! ```
//!
//! Creates an association between a notification rule and an Amazon Q Developer in chat
//! applications topic or Amazon Q Developer in chat applications client so that the
//! associated target can receive notifications when the events described in the rule are
//! triggered.
//!
//! ## Input
//!
//! * `Arn` (NotificationRuleArn, pattern
//!   ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$)
//!   **required** — The Amazon Resource Name (ARN) of the notification rule for which you
//!   want to create the association.
//! * `ClientRequestToken` (ClientRequestToken, len 1..256, pattern ^[\w:/-]+$) — An
//!   enumeration token that, when provided in a request, returns the next batch of the
//!   results.
//! * `Target` (Target) **required** — Information about the Amazon Q Developer in chat
//!   applications topics or Amazon Q Developer in chat applications clients associated with a
//!   notification rule.
//!
//! ## Output
//!
//! * `Arn` (NotificationRuleArn, pattern
//!   ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$) — The
//!   Amazon Resource Name (ARN) of the notification rule for which you have created
//!   assocations.
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
// `SubscribeError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `SubscribeError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `AccessDeniedException` is widened in through `extra_errors`: this SERVICE models the
// shape (`@httpError 403`, "you do not have sufficient permissions") and it is the answer
// the resource-based authorization check gives an `Arn` that names no notification rule,
// which every operation addressing a rule by ARN applies before it looks one up.
#[smithy_macros::operation_error(
    "com.amazonaws.codestarnotifications#Subscribe",
    extra_errors = ["AccessDeniedException"]
)]
#[derive(Debug)]
pub enum SubscribeErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConfigurationException(ConfigurationException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::subscribe::SubscribeError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type SubscribeError = crate::scaffolded::Scaffolded<SubscribeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<SubscribeErrorModeled> for SubscribeError {
    fn from(modeled: SubscribeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// HTTP 400 `ConfigurationException`: "Some or all of the configuration is incomplete,
/// missing, or not valid."
fn misconfigured(message: impl Into<String>) -> SubscribeError {
    SubscribeErrorModeled::ConfigurationException(ConfigurationException {
        message: Some(message.into()),
    })
    .into()
}

/// HTTP 403 `AccessDeniedException`: the caller is not authorized for the resource the
/// request named, stated in this service's one text for the class.
fn not_authorized(arn: &str) -> SubscribeError {
    SubscribeErrorModeled::AccessDeniedException(AccessDeniedException {
        message: Some(crate::state::not_authorized_for(arn)),
    })
    .into()
}

/// The `Subscribe` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct Subscribe<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> Subscribe<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for Subscribe<Eng>
{
    type Input = SubscribeInput;
    type Output = SubscribeOutput;
    type Error = SubscribeError;

    const NAME: &'static str = "Subscribe";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SUBSCRIBE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SUBSCRIBE_OUTPUT_SCHEMA;
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

        let target = st::rule_targets_from(std::slice::from_ref(&input.target))
            .map_err(misconfigured)?
            .into_iter()
            .next()
            .ok_or_else(|| misconfigured("Target is required"))?;
        let fingerprint = st::fingerprint(&[
            input.arn.clone(),
            target.target_type.clone(),
            target.target_address.clone(),
        ]);

        let Some(state) = st::service_state(&*self.engine) else {
            // This service's store is the shared `InMemoryEngine`. Mounted on any other
            // engine there is no notification-rule store to subscribe on, which is a
            // "not written for this engine" condition, not a client error.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "Subscribe (no state store on this engine)",
            ));
        };
        let mut store = state.lock();

        if let Some(token) = input.client_request_token.as_deref() {
            if let Some(previous) = store.idempotent_request(Self::NAME, token) {
                if previous.fingerprint != fingerprint {
                    return Err(SubscribeErrorModeled::ValidationException(
                        ValidationException {
                            message: Some(format!(
                                "ClientRequestToken {token} was already used with different parameters"
                            )),
                        },
                    )
                    .into());
                }
                return Ok(SubscribeOutput {
                    arn: Some(previous.arn.clone()),
                });
            }
        }

        // Upsert: an address already subscribed to this rule is one subscription, whose
        // type the new call re-states; a new address is appended, up to the modelled
        // maximum the rule's `Targets` list carries.
        let arn = input.arn.clone();
        if store.rule_in_region(&region, &arn).is_none() {
            // This operation names the missing rule by the whole ARN it was given.
            return Err(SubscribeErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(st::no_rule_with_identifier(&arn)),
                },
            )
            .into());
        }
        {
            let Some(rule) = store.rules.get_mut(&arn) else {
                return Err(SubscribeErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(st::no_rule_with_identifier(&arn)),
                    },
                )
                .into());
            };
            match rule
                .targets
                .iter_mut()
                .find(|existing| existing.target_address == target.target_address)
            {
                Some(existing) => existing.target_type = target.target_type.clone(),
                None => {
                    if rule.targets.len() >= st::MAX_TARGETS_PER_RULE {
                        return Err(misconfigured(format!(
                            "A notification rule carries at most {} targets",
                            st::MAX_TARGETS_PER_RULE
                        )));
                    }
                    rule.targets.push(target.clone());
                }
            }
            rule.last_modified_timestamp = st::now_timestamp();
        }
        store.register_target(&arn, &target);
        if let Some(token) = input.client_request_token.as_deref() {
            store.remember_idempotent_request(
                Self::NAME,
                token,
                st::IdempotentRequest {
                    arn: arn.clone(),
                    fingerprint,
                },
            );
        }

        Ok(SubscribeOutput { arn: Some(arn) })
    }
}
