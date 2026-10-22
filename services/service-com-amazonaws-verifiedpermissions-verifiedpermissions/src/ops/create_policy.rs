// SPDX-License-Identifier: CC0-1.0

//! @op CreatePolicy
//!
//! Stores a Cedar policy in a policy store. The `static` arm keeps the caller's
//! Cedar text verbatim; `effect`, `principal`, `resource` and `actions` are read
//! off the statement by the real Cedar parser
//! ([`crate::state::cedar::parse_static_policy`]), never pattern-matched out of
//! the text, and every response field is read back out of what was persisted.
//!
//! The `templateLinked` arm instantiates a policy template, which is its own
//! operation slice, so it answers the scaffold's 501 rather than a fabricated
//! policy.
//!
//! A statement this operation refuses — unparseable Cedar, longer than one policy
//! may carry, or not sanctioned by the store's schema in `STRICT` mode — is
//! refused with the model's own `ValidationException`, whose `fieldList` names
//! `/definition/static/statement`, the member at fault. See
//! [`crate::ops::validation_exception`].
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreatePolicy`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Creates a Cedar policy and saves it in the specified policy store. You can create either
//! a static policy or a policy linked to a policy template. To create a static policy,
//! provide the Cedar policy text in the StaticPolicy section of the PolicyDefinition. To
//! create a policy that is dynamically linked to a policy template, specify the policy
//! template ID and the principal and resource to associate with this policy in the
//! templateLinked section of the PolicyDefinition. If the policy template is ever updated,
//! any policies linked to the policy template automatically use the updated template.
//! Creating a policy causes it to be validated against the schema in the policy store. If
//! the policy doesn't pass validation, the operation fails and the policy isn't stored.
//! Verified Permissions is eventually consistent . It can take a few seconds for a new or
//! changed element to propagate through the service and be visible in the results of other
//! Verified Permissions operations.
//!
//! ## Input
//!
//! * `clientToken` (IdempotencyToken, len 1..64, pattern ^[a-zA-Z0-9-]*$, @idempotencyToken)
//!   — Specifies a unique, case-sensitive ID that you provide to ensure the idempotency of
//!   the request. This lets you safely retry the request without accidentally performing the
//!   same operation a second time. Passing the same value to a later call to an operation
//!   requires that you also pass the same value for all other parameters. We recommend that
//!   you use a UUID type of value.. If you don't provide this value, then Amazon Web Services
//!   generates a random one for you. If you retry the operation with the same ClientToken,
//!   but with different parameters, the retry fails with an ConflictException error. Verified
//!   Permissions recognizes a ClientToken for eight hours. After eight hours, the next
//!   request …
//! * `definition` (PolicyDefinition) **required** — A structure that specifies the policy
//!   type and content to use for the new policy. You must include either a static or a
//!   templateLinked element. The policy content must be written in the Cedar policy language.
//! * `name` (PolicyName, len 0..150, pattern ^[a-zA-Z0-9-/_]*$) — Specifies a name for the
//!   policy that is unique among all policies within the policy store. You can use the name
//!   in place of the policy ID in API operations that reference the policy. The name must be
//!   prefixed with name/. If you specify a name that is already associated with another
//!   policy in the policy store, you receive a ConflictException error.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the PolicyStoreId of the policy store you want to store the policy in. To
//!   specify a policy store, use its ID or alias name. When using an alias name, prefix it
//!   with policy-store-alias/. For example: ID: PSEXAMPLEabcdefg111111 Alias name:
//!   policy-store-alias/example-policy-store To view aliases, use ListPolicyStoreAliases.
//!
//! ## Output
//!
//! * `actions` (ActionIdentifierList) — The action that a policy permits or forbids. For
//!   example, {"actions": [{"actionId": "ViewPhoto", "actionType": "PhotoFlash::Action"},
//!   {"entityID": "SharePhoto", "entityType": "PhotoFlash::Action"}]}.
//! * `createdDate` (TimestampFormat) **required** — The date and time the policy was
//!   originally created.
//! * `effect` (PolicyEffect, one of: Forbid | Permit) — The effect of the decision that a
//!   policy returns to an authorization request. For example, "effect": "Permit".
//! * `lastUpdatedDate` (TimestampFormat) **required** — The date and time the policy was last
//!   updated.
//! * `policyId` (PolicyId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** — The unique
//!   ID of the new policy.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   The ID of the policy store that contains the new policy.
//! * `policyType` (PolicyType, one of: STATIC | TEMPLATE_LINKED) **required** — The policy
//!   type of the new policy.
//! * `principal` (EntityIdentifier) — The principal specified in the new policy's scope. This
//!   response element isn't present when principal isn't specified in the policy content.
//! * `resource` (EntityIdentifier) — The resource specified in the new policy's scope. This
//!   response element isn't present when the resource isn't specified in the policy content.
//!
//! ## Errors
//!
//! * `ConflictException` (HTTP 409) — The request failed because another request to modify a
//!   resource occurred at the same time.
//! * `ResourceNotFoundException` (HTTP 404) — The request failed because it references a
//!   resource that doesn't exist.
//! * `ServiceQuotaExceededException` (HTTP 402) — The request failed because it would cause a
//!   service quota to be exceeded.
//! * `ValidationException` (HTTP 400) — The request failed because one or more input
//!   parameters don't satisfy their constraint requirements; `fieldList` names each
//!   member that isn't valid and why.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreatePolicyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreatePolicyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#CreatePolicy",
    extra_errors = ["InternalServerException", "ValidationException"]
)]
#[derive(Debug)]
pub enum CreatePolicyErrorModeled {
    ConflictException(ConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_policy::CreatePolicyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreatePolicyError = crate::scaffolded::Scaffolded<CreatePolicyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreatePolicyErrorModeled> for CreatePolicyError {
    fn from(modeled: CreatePolicyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreatePolicy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreatePolicy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreatePolicy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreatePolicy<Eng>
{
    type Input = CreatePolicyInput;
    type Output = CreatePolicyOutput;
    type Error = CreatePolicyError;

    const NAME: &'static str = "CreatePolicy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_POLICY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_POLICY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let not_found = || {
            CreatePolicyErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };
        let unavailable = || {
            CreatePolicyErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        };
        // Every refusal of the caller's policy text answers with the model's own
        // `ValidationException`, whose `fieldList` names the member at fault —
        // here always `definition.static.statement`, the one member whose content
        // this operation validates. A refusal the CALLER cannot fix (the store's
        // own stored schema failing to parse) is a server fault instead.
        let invalid = |invalid: crate::ops::InvalidInput| -> CreatePolicyError {
            CreatePolicyErrorModeled::ValidationException(crate::ops::validation_exception([
                invalid,
            ]))
            .into()
        };
        let refused = |refusal: crate::state::cedar::Refusal| -> CreatePolicyError {
            match refusal {
                crate::state::cedar::Refusal::InvalidInput(field) => invalid(field),
                crate::state::cedar::Refusal::ServiceFault(message) => {
                    CreatePolicyErrorModeled::InternalServerException(InternalServerException {
                        message,
                    })
                    .into()
                }
            }
        };

        // Only the `static` arm is served here: instantiating a template into a
        // policy (resolving its `?principal`/`?resource` slots and linking the
        // policy to the template) is its own slice, so there is nothing honest to
        // return for `templateLinked` yet.
        let definition = match &input.definition {
            PolicyDefinition::Static(def) => def.clone(),
            PolicyDefinition::templateLinked(_) => {
                return Err(crate::scaffolded::Scaffolded::not_implemented(
                    "CreatePolicy with a templateLinked definition",
                ));
            }
        };

        // The SIZE BOUND on everything below: the cheap length check, before the
        // state lock and before any Cedar parse or schema validation, so no
        // caller-sized statement reaches the parser or the validator. Only the
        // length is checked here — the full parse stays where it was, after the
        // store is resolved, so an unknown store is still a 404 and a client
        // token still replays before the statement is parsed.
        crate::state::cedar::check_statement_size(&definition.statement).map_err(invalid)?;

        let state = crate::state::service_state(&*self.engine).ok_or_else(unavailable)?;
        let mut state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;

        // `@idempotencyToken` replay: the same token answers with the policy the
        // first call created; the same token with different parameters conflicts.
        let parameters = crate::state::idempotency_parameters(&[
            ("store", Some(store_id.as_str())),
            ("name", input.name.as_deref()),
            ("statement", Some(definition.statement.as_str())),
            ("description", definition.description.as_deref()),
        ]);
        if let Some(token) = input.client_token.as_deref() {
            if let Some(record) = state.idempotency_replay("CreatePolicy", token) {
                if record.parameters != parameters {
                    // Fixed message; the policy the token already created is
                    // reported in `resources`.
                    return Err(CreatePolicyErrorModeled::ConflictException(
                        crate::ops::conflict([crate::ops::resource_conflict(
                            &record.resource_id,
                            crate::state::RESOURCE_TYPE_POLICY,
                        )]),
                    )
                    .into());
                }
                if let Some(policy) = state
                    .policy_stores
                    .get(&store_id)
                    .and_then(|store| store.policies.get(&record.resource_id))
                {
                    return Ok(output_for(policy));
                }
            }
        }

        // `resolve_store_id` answers only for a store that is present, so this is
        // a lookup of a key it just confirmed — but it is still resolved through
        // the error channel rather than an `expect`, because the key came from the
        // request and a handler must not be able to panic on caller input.
        let store = state.policy_stores.get(&store_id).ok_or_else(not_found)?;

        // `ValidationSettings` documents this exactly: "STRICT — Requires a schema
        // to be present in the policy store. Cedar performs validation on all
        // submitted new or updated static policies and policy templates. Any that
        // fail validation are rejected and Cedar doesn't store them in the policy
        // store", and "If Mode=STRICT and the policy store doesn't contain a
        // schema, Verified Permissions rejects all static policies and policy
        // templates because there is no schema to validate against."
        if store.validation_mode == "STRICT" {
            match store.schema.as_ref() {
                None => {
                    // Nothing is wrong with the statement here: the STORE the
                    // request named holds no schema, which is the member the
                    // caller can act on (put a schema in it, or name a store
                    // that has one).
                    return Err(invalid(crate::ops::InvalidInput::new(
                        crate::ops::paths::POLICY_STORE_ID,
                        "its validation mode is STRICT and it contains no schema, so there \
                         is nothing to validate a static policy against",
                    )));
                }
                Some(schema) => {
                    crate::state::cedar::validate_static_policy(
                        &schema.cedar_json,
                        &definition.statement,
                    )
                    .map_err(refused)?;
                }
            }
        }

        if store.policies.len() >= crate::state::MAX_POLICIES_PER_STORE
            || store.cedar_bytes() + definition.statement.len()
                > crate::state::MAX_POLICY_BYTES_PER_STORE
        {
            return Err(CreatePolicyErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: format!(
                        "policy store {store_id} is at its limit of {} policies / {} bytes \
                         of Cedar text",
                        crate::state::MAX_POLICIES_PER_STORE,
                        crate::state::MAX_POLICY_BYTES_PER_STORE
                    ),
                    resource_id: Some(store_id.clone()),
                    resource_type: "POLICY".to_string(),
                    ..Default::default()
                },
            )
            .into());
        }

        // A policy name is unique within its store.
        if let Some(name) = input.name.as_deref() {
            if let Some(clash) = store
                .policies
                .values()
                .find(|p| p.name.as_deref() == Some(name))
            {
                return Err(CreatePolicyErrorModeled::ConflictException(
                    crate::ops::conflict([crate::ops::resource_conflict(
                        &clash.policy_id,
                        crate::state::RESOURCE_TYPE_POLICY,
                    )]),
                )
                .into());
            }
        }

        // The effect and the scope are properties of the Cedar text, so they come
        // from the Cedar parser. A statement it refuses is refused here too.
        let scope = crate::state::cedar::parse_static_policy(&definition.statement)
            .map_err(invalid)?;

        let store = state
            .policy_stores
            .get_mut(&store_id)
            .ok_or_else(not_found)?;
        let policy_id = store.mint_policy_id();
        let created = crate::state::now();
        let policy = crate::state::Policy {
            policy_id: policy_id.clone(),
            policy_store_id: store_id.clone(),
            policy_type: "STATIC".to_string(),
            name: input.name.clone(),
            statement: definition.statement.clone(),
            description: definition.description.clone(),
            effect: Some(scope.effect),
            principal: scope.principal,
            resource: scope.resource,
            actions: scope.actions,
            created_date: created,
            last_updated_date: created,
        };
        store.policies.insert(policy_id.clone(), policy);
        if let Some(token) = input.client_token.as_deref() {
            state.record_idempotency("CreatePolicy", token, &policy_id, parameters);
        }

        // Read the response back out of persisted state.
        let stored = state
            .policy_stores
            .get(&store_id)
            .and_then(|store| store.policies.get(&policy_id))
            .ok_or_else(unavailable)?;
        let output = output_for(stored);
        crate::ops::audit(ctx, "CreatePolicy", &output.policy_id, "created");
        Ok(output)
    }
}

/// Render a persisted policy as this operation's output. Every member is read out
/// of state, so a value that was never stored cannot appear on the wire.
fn output_for(policy: &crate::state::Policy) -> CreatePolicyOutput {
    CreatePolicyOutput {
        policy_store_id: policy.policy_store_id.clone(),
        policy_id: policy.policy_id.clone(),
        policy_type: policy.policy_type.clone(),
        principal: policy.principal.as_ref().map(entity_identifier),
        resource: policy.resource.as_ref().map(entity_identifier),
        actions: (!policy.actions.is_empty())
            .then(|| policy.actions.iter().map(action_identifier).collect()),
        created_date: policy.created_date,
        last_updated_date: policy.last_updated_date,
        effect: policy.effect.clone(),
    }
}

/// A persisted scope entity as the modeled `EntityIdentifier`.
pub(crate) fn entity_identifier(entity: &crate::state::ScopeEntity) -> EntityIdentifier {
    EntityIdentifier {
        entity_type: entity.entity_type.clone(),
        entity_id: entity.entity_id.clone(),
    }
}

/// A persisted scope entity as the modeled `ActionIdentifier`.
pub(crate) fn action_identifier(entity: &crate::state::ScopeEntity) -> ActionIdentifier {
    ActionIdentifier {
        action_type: entity.entity_type.clone(),
        action_id: entity.entity_id.clone(),
    }
}
