// SPDX-License-Identifier: CC0-1.0

//! @op CreateNotificationRule
//!
//! Mints a notification rule and persists it in [`crate::state`]: the rule's id and
//! ARN, its name (unique per account and region), resource, detail type, status, event type ids,
//! targets and tags. The ARN it returns is the key `DescribeNotificationRule`,
//! `Subscribe`, `UpdateNotificationRule` and `DeleteNotificationRule` read back.
//!
//! The engine is held on `self` (constructor-injected); `ctx` carries request metadata
//! only — the request's SigV4 scope is where the ARN's region and the rule's
//! `CreatedBy` principal come from.
//!
//! Two input rules the MODEL cannot express are applied here, before anything is
//! inserted: `Resource` must name a resource of a type this service supports, and every
//! `EventTypeIds` entry must be an event type the service publishes (see
//! [`crate::state::SUPPORTED_RESOURCE_SERVICES`] and
//! [`crate::state::event_type_ids_violation`]).
//!
//! ---
//!
//! # Modeled contract for `CreateNotificationRule`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json")
//! ```
//!
//! Creates a notification rule for a resource. The rule specifies the events you want
//! notifications about and the targets (such as Amazon Q Developer in chat applications
//! topics or Amazon Q Developer in chat applications clients configured for Slack) where
//! you want to receive them.
//!
//! ## Input
//!
//! * `ClientRequestToken` (ClientRequestToken, len 1..256, pattern ^[\w:/-]+$,
//!   @idempotencyToken) — A unique, client-generated idempotency token that, when provided in
//!   a request, ensures the request cannot be repeated with a changed parameter. If a request
//!   with the same parameters is received and a token is included, the request returns
//!   information about the initial request that used that token. The Amazon Web Services SDKs
//!   prepopulate client request tokens. If you are using an Amazon Web Services SDK, an
//!   idempotency token is created for you.
//! * `DetailType` (DetailType, one of: BASIC | FULL) **required** — The level of detail to
//!   include in the notifications for this resource. BASIC will include only the contents of
//!   the event as it would appear in Amazon CloudWatch. FULL will include any supplemental
//!   information provided by CodeStar Notifications and/or the service for the resource for
//!   which the notification is created.
//! * `EventTypeIds` (EventTypeIds) **required** — A list of event types associated with this
//!   notification rule. For a list of allowed events, see EventTypeSummary.
//! * `Name` (NotificationRuleName, len 1..64, pattern ^[A-Za-z0-9\-_ ]+$) **required** — The
//!   name for the notification rule. Notification rule names must be unique in your Amazon
//!   Web Services account.
//! * `Resource` (NotificationRuleResource, pattern
//!   ^arn:aws[^:\s]*:[^:\s]*:[^:\s]*:[0-9]{12}:[^\s]+$) **required** — The Amazon Resource
//!   Name (ARN) of the resource to associate with the notification rule. Supported resources
//!   include pipelines in CodePipeline, repositories in CodeCommit, and build projects in
//!   CodeBuild.
//! * `Status` (NotificationRuleStatus, one of: DISABLED | ENABLED) — The status of the
//!   notification rule. The default value is ENABLED. If the status is set to DISABLED,
//!   notifications aren't sent for the notification rule.
//! * `Tags` (Tags) — A list of tags to apply to this notification rule. Key names cannot
//!   start with "aws".
//! * `Targets` (Targets, len 0..10) **required** — A list of Amazon Resource Names (ARNs) of
//!   Amazon Simple Notification Service topics and Amazon Q Developer in chat applications
//!   clients to associate with the notification rule.
//!
//! ## Output
//!
//! * `Arn` (NotificationRuleArn, pattern
//!   ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$) — The
//!   Amazon Resource Name (ARN) of the notification rule.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — CodeStar Notifications can't create the
//!   notification rule because you do not have sufficient permissions.
//! * `ConcurrentModificationException` (HTTP 400) — CodeStar Notifications can't complete the
//!   request because the resource is being modified by another process. Wait a few minutes
//!   and try again.
//! * `ConfigurationException` (HTTP 400) — Some or all of the configuration is incomplete,
//!   missing, or not valid.
//! * `LimitExceededException` (HTTP 400) — One of the CodeStar Notifications limits has been
//!   exceeded. Limits apply to accounts, notification rules, notifications, resources, and
//!   targets. For more information, see Limits.
//! * `ResourceAlreadyExistsException` (HTTP 409) — A resource with the same name or ID
//!   already exists. Notification rule names must be unique in your Amazon Web Services
//!   account.
//! * `ValidationException` (HTTP 400) — One or more parameter values are not valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateNotificationRuleError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateNotificationRuleError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.codestarnotifications#CreateNotificationRule")]
#[derive(Debug)]
pub enum CreateNotificationRuleErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConcurrentModificationException(ConcurrentModificationException),
    ConfigurationException(ConfigurationException),
    LimitExceededException(LimitExceededException),
    ResourceAlreadyExistsException(ResourceAlreadyExistsException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_notification_rule::CreateNotificationRuleError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateNotificationRuleError = crate::scaffolded::Scaffolded<CreateNotificationRuleErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateNotificationRuleErrorModeled> for CreateNotificationRuleError {
    fn from(modeled: CreateNotificationRuleErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// HTTP 400 `ValidationException`, the modelled shape — "One or more parameter values
/// are not valid" — naming the member that is wrong.
fn invalid(message: impl Into<String>) -> CreateNotificationRuleError {
    CreateNotificationRuleErrorModeled::ValidationException(ValidationException {
        message: Some(message.into()),
    })
    .into()
}

/// HTTP 400 `ConfigurationException`: "Some or all of the configuration is incomplete,
/// missing, or not valid" — a target that does not carry both of its members.
fn misconfigured(message: impl Into<String>) -> CreateNotificationRuleError {
    CreateNotificationRuleErrorModeled::ConfigurationException(ConfigurationException {
        message: Some(message.into()),
    })
    .into()
}

/// The `CreateNotificationRule` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateNotificationRule<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateNotificationRule<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateNotificationRule<Eng>
{
    type Input = CreateNotificationRuleInput;
    type Output = CreateNotificationRuleOutput;
    type Error = CreateNotificationRuleError;

    const NAME: &'static str = "CreateNotificationRule";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_NOTIFICATION_RULE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_NOTIFICATION_RULE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state as st;

        let region = st::request_region(ctx.parts());
        let created_by = st::caller_principal_arn(ctx.parts());

        if let Some(reason) =
            st::enum_violation("DetailType", &input.detail_type, DETAIL_TYPE_VALUES)
        {
            return Err(invalid(reason));
        }
        let status = input
            .status
            .clone()
            .unwrap_or_else(|| st::DEFAULT_RULE_STATUS.to_string());
        if let Some(reason) =
            st::enum_violation("Status", &status, NOTIFICATION_RULE_STATUS_VALUES)
        {
            return Err(invalid(reason));
        }
        // The resource a rule fires for must be of a type this service supports: the
        // modelled pattern accepts any service's ARN, so the type restriction is the
        // service's and is enforced here, with the service's own fixed text.
        if !st::resource_type_is_supported(&input.resource) {
            return Err(invalid(st::UNSUPPORTED_RESOURCE_TYPE));
        }
        // And every event type must be one the service publishes, so a rule cannot be
        // created wired to an event that does not exist.
        if let Some(reason) = st::event_type_ids_violation(&input.event_type_ids) {
            return Err(invalid(reason));
        }
        let targets = st::rule_targets_from(&input.targets).map_err(misconfigured)?;
        let tags = st::rule_tags_from(input.tags.as_ref()).map_err(invalid)?;
        // A rule holds a bounded number of tags, for the same reason the store holds a
        // bounded number of rules: tagging is caller-driven growth of a stored resource.
        if tags.len() > st::MAX_TAGS_PER_RULE {
            return Err(
                CreateNotificationRuleErrorModeled::LimitExceededException(
                    LimitExceededException {
                        message: Some(format!(
                            "A notification rule holds at most {} tags",
                            st::MAX_TAGS_PER_RULE
                        )),
                    },
                )
                .into(),
            );
        }

        // The parameters this token is allowed to replay: anything else under the same
        // token is the "repeated with a changed parameter" the member documents.
        let mut fields = vec![
            input.name.clone(),
            input.resource.clone(),
            input.detail_type.clone(),
            status.clone(),
        ];
        let mut sorted_event_type_ids = input.event_type_ids.clone();
        sorted_event_type_ids.sort();
        fields.extend(sorted_event_type_ids);
        fields.extend(
            targets
                .iter()
                .map(|t| format!("{}={}", t.target_type, t.target_address)),
        );
        fields.extend(tags.iter().map(|(k, v)| format!("{k}={v}")));
        let fingerprint = st::fingerprint(&fields);

        let Some(state) = st::service_state(&*self.engine) else {
            // This service's store is the shared `InMemoryEngine`. Mounted on any other
            // engine there is no notification-rule store to serve from, which is a
            // "not written for this engine" condition, not a client error.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "CreateNotificationRule (no state store on this engine)",
            ));
        };
        let mut store = state.lock();

        if let Some(token) = input.client_request_token.as_deref() {
            if let Some(previous) = store.idempotent_request(Self::NAME, token) {
                if previous.fingerprint != fingerprint {
                    return Err(invalid(format!(
                        "ClientRequestToken {token} was already used with different parameters"
                    )));
                }
                // Same token, same parameters: report the rule that token created.
                return Ok(CreateNotificationRuleOutput {
                    arn: Some(previous.arn.clone()),
                });
            }
        }

        if store
            .rule_names
            .contains_key(&st::rule_name_key(&region, &input.name))
        {
            return Err(
                CreateNotificationRuleErrorModeled::ResourceAlreadyExistsException(
                    ResourceAlreadyExistsException {
                        // The service's own fixed text for this error class. It states
                        // the name-uniqueness rule and nothing of the request or of the
                        // rule that holds the name — see `st::RULE_NAME_ALREADY_IN_USE`.
                        message: Some(st::RULE_NAME_ALREADY_IN_USE.to_string()),
                    },
                )
                .into(),
            );
        }

        // The store holds a bounded number of rules per account and region, so a caller
        // cannot grow it without limit; the model declares `LimitExceededException` for
        // exactly this ("Limits apply to accounts, notification rules, ...").
        if store.rule_count_in_region(&region) >= st::MAX_RULES_PER_REGION {
            return Err(
                CreateNotificationRuleErrorModeled::LimitExceededException(
                    LimitExceededException {
                        message: Some(format!(
                            "This account already holds the maximum of {} notification rules in {region}",
                            st::MAX_RULES_PER_REGION
                        )),
                    },
                )
                .into(),
            );
        }

        // And a bound that is keyed on nothing the request chooses: the region a rule
        // lands in comes from the caller's own credential scope, so the per-region cap
        // above cannot bound the store by itself.
        if store.rule_count() >= st::MAX_RULES_TOTAL {
            return Err(
                CreateNotificationRuleErrorModeled::LimitExceededException(
                    LimitExceededException {
                        message: Some(format!(
                            "This account already holds the maximum of {} notification rules",
                            st::MAX_RULES_TOTAL
                        )),
                    },
                )
                .into(),
            );
        }

        let sequence = store.take_sequence();
        let id = st::new_rule_id(sequence);
        let arn = st::notification_rule_arn(&region, &id);
        let created_timestamp = st::now_timestamp();
        let rule = st::NotificationRule {
            id,
            arn: arn.clone(),
            region: region.clone(),
            name: input.name.clone(),
            resource: input.resource.clone(),
            detail_type: input.detail_type.clone(),
            status,
            event_type_ids: input.event_type_ids.clone(),
            targets: targets.clone(),
            tags,
            created_by,
            created_timestamp,
            last_modified_timestamp: created_timestamp,
            sequence,
        };
        for target in &targets {
            store.register_target(&arn, target);
        }
        store
            .rule_names
            .insert(st::rule_name_key(&region, &input.name), arn.clone());
        store.rules.insert(arn.clone(), rule);
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

        Ok(CreateNotificationRuleOutput { arn: Some(arn) })
    }
}
