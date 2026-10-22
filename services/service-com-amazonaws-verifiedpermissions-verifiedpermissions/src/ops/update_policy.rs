// SPDX-License-Identifier: CC0-1.0

//! @op UpdatePolicy
//!
//! Replaces the content of a STATIC policy. The replacement text is parsed by
//! the real Cedar parser ([`crate::state::cedar::parse_static_policy`]) and the
//! elements the API documents as immutable — the effect, the principal and the
//! resource of the policy's scope, and the kind of policy it is — are compared
//! against what the stored policy holds: a replacement that changes one of them
//! is refused with the model's own `ValidationException` and nothing is stored.
//! What may change is what the docs name: the action the policy references and
//! its condition clauses.
//!
//! A template-linked policy has no content of its own to replace — its text is
//! its template's — so it is refused here too: "To change a template-linked
//! policy, you must update the template instead, using UpdatePolicyTemplate."
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdatePolicy`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Modifies a Cedar static policy in the specified policy store. You can change only
//! certain elements of the UpdatePolicyDefinition parameter. You can directly update only
//! static policies. To change a template-linked policy, you must update the template
//! instead, using UpdatePolicyTemplate. If policy validation is enabled in the policy
//! store, then updating a static policy causes Verified Permissions to validate the policy
//! against the schema in the policy store. If the updated static policy doesn't pass
//! validation, the operation fails and the update isn't stored. When you edit a static
//! policy, you can change only certain elements of a static policy: The action referenced
//! by the policy. A condition clause, such as when and unless. You can't change these
//! elements of a static policy: Changing a policy from a static policy to a template-linked
//! policy. Changing the effect of a static policy from permit or forbid. The principal
//! referenced by a static policy. The resource referenced by a static policy. To update a
//! template-linked policy, you must update the template instead. Verified Permissions is
//! eventually consistent . It can take a few seconds for a new or changed element to
//! propagate through the service and be visible in the results of other Verified
//! Permissions operations.
//!
//! ## Input
//!
//! * `definition` (UpdatePolicyDefinition) — Specifies the updated policy content that you
//!   want to replace on the specified policy. The content must be valid Cedar policy language
//!   text. If you don't specify this parameter, the existing policy definition remains
//!   unchanged. You can change only the following elements from the policy definition: The
//!   action referenced by the policy. Any conditional clauses, such as when or unless
//!   clauses. You can't change the following elements: Changing from static to
//!   templateLinked. Changing the effect of the policy from permit or forbid. The principal
//!   referenced by the policy. The resource referenced by the policy.
//! * `name` (PolicyName, len 0..150, pattern ^[a-zA-Z0-9-/_]*$) — Specifies a name for the
//!   policy that is unique among all policies within the policy store. You can use the name
//!   in place of the policy ID in API operations that reference the policy. The name must be
//!   prefixed with name/. If you don't include the name in an update request, the existing
//!   name is unchanged. To remove a name, set it to an empty string (""). If you specify a
//!   name that is already associated with another policy in the policy store, you receive a
//!   ConflictException error.
//! * `policyId` (PolicyId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** — Specifies
//!   the ID of the policy that you want to update. To find this value, you can use
//!   ListPolicies. You can use the policy name in place of the policy ID. When using a name,
//!   prefix it with name/. For example: ID: SPEXAMPLEabcdefg111111 Name: name/example-policy
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store that contains the policy that you want to update.
//!   To specify a policy store, use its ID or alias name. When using an alias name, prefix it
//!   with policy-store-alias/. For example: ID: PSEXAMPLEabcdefg111111 Alias name:
//!   policy-store-alias/example-policy-store To view aliases, use ListPolicyStoreAliases.
//!
//! ## Output
//!
//! * `actions` (ActionIdentifierList) — The action that a policy permits or forbids. For
//!   example, {"actions": [{"actionId": "ViewPhoto", "actionType": "PhotoFlash::Action"},
//!   {"entityID": "SharePhoto", "entityType": "PhotoFlash::Action"}]}.
//! * `createdDate` (TimestampFormat) **required** — The date and time that the policy was
//!   originally created.
//! * `effect` (PolicyEffect, one of: Forbid | Permit) — The effect of the decision that a
//!   policy returns to an authorization request. For example, "effect": "Permit".
//! * `lastUpdatedDate` (TimestampFormat) **required** — The date and time that the policy was
//!   most recently updated.
//! * `policyId` (PolicyId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** — The ID of
//!   the policy that was updated.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   The ID of the policy store that contains the policy that was updated.
//! * `policyType` (PolicyType, one of: STATIC | TEMPLATE_LINKED) **required** — The type of
//!   the policy that was updated.
//! * `principal` (EntityIdentifier) — The principal specified in the policy's scope. This
//!   element isn't included in the response when Principal isn't present in the policy
//!   content.
//! * `resource` (EntityIdentifier) — The resource specified in the policy's scope. This
//!   element isn't included in the response when Resource isn't present in the policy
//!   content.
//!
//! ## Errors
//!
//! * `ConflictException` (HTTP 409) — The request failed because another request to modify a
//!   resource occurred at the same time.
//! * `ResourceNotFoundException` (HTTP 404) — The request failed because it references a
//!   resource that doesn't exist.
//! * `ServiceQuotaExceededException` (HTTP 402) — The request failed because it would cause a
//!   service quota to be exceeded.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdatePolicyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdatePolicyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#UpdatePolicy",
    extra_errors = ["InternalServerException", "ValidationException"]
)]
#[derive(Debug)]
pub enum UpdatePolicyErrorModeled {
    ConflictException(ConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_policy::UpdatePolicyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdatePolicyError = crate::scaffolded::Scaffolded<UpdatePolicyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdatePolicyErrorModeled> for UpdatePolicyError {
    fn from(modeled: UpdatePolicyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdatePolicy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdatePolicy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdatePolicy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdatePolicy<Eng>
{
    type Input = UpdatePolicyInput;
    type Output = UpdatePolicyOutput;
    type Error = UpdatePolicyError;

    const NAME: &'static str = "UpdatePolicy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_POLICY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_POLICY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store_not_found = || {
            UpdatePolicyErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };
        let policy_not_found = || {
            UpdatePolicyErrorModeled::ResourceNotFoundException(
                crate::state::policy_reference(&input.policy_id).not_found(),
            )
        };
        let unavailable = || {
            UpdatePolicyErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        };
        let refuse = |invalid: Vec<crate::ops::InvalidInput>| -> UpdatePolicyError {
            UpdatePolicyErrorModeled::ValidationException(crate::ops::validation_exception(
                invalid,
            ))
            .into()
        };
        let invalid = |invalid: crate::ops::InvalidInput| -> UpdatePolicyError {
            refuse(vec![invalid])
        };
        let refused = |refusal: crate::state::cedar::Refusal| -> UpdatePolicyError {
            match refusal {
                crate::state::cedar::Refusal::InvalidInput(field) => invalid(field),
                // The store's SCHEMA refused the policy this definition
                // describes: one `fieldList` entry per validation reason, all of
                // them naming `/definition`, the member whose content was
                // validated.
                crate::state::cedar::Refusal::SchemaValidation(reasons) => {
                    UpdatePolicyErrorModeled::ValidationException(
                        crate::ops::schema_validation_exception(
                            crate::ops::paths::DEFINITION,
                            reasons,
                        ),
                    )
                    .into()
                }
                crate::state::cedar::Refusal::ServiceFault(message) => {
                    UpdatePolicyErrorModeled::InternalServerException(InternalServerException {
                        message,
                    })
                    .into()
                }
            }
        };

        // The SIZE BOUND first, before the state lock and before any Cedar parse
        // or schema validation, so no caller-sized statement reaches the parser.
        if let Some(UpdatePolicyDefinition::Static(def)) = &input.definition {
            crate::state::cedar::check_statement_size(&def.statement).map_err(invalid)?;
        }

        let state = crate::state::service_state(&*self.engine).ok_or_else(unavailable)?;
        let mut state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(store_not_found)?;
        let store = state
            .policy_stores
            .get(&store_id)
            .ok_or_else(store_not_found)?;
        let policy_id = store
            .resolve_policy_id(&input.policy_id)
            .ok_or_else(policy_not_found)?;
        let policy = store
            .policies
            .get(&policy_id)
            .ok_or_else(policy_not_found)?;

        // What the replacement resolves to, when the request carries one: the
        // parsed scope of the new text. "If you don't specify this parameter,
        // the existing policy definition remains unchanged", so an absent
        // `definition` leaves the content alone and only `name` can change.
        let replacement = match &input.definition {
            None => None,
            Some(UpdatePolicyDefinition::Static(def)) => {
                // "You can directly update only static policies. To change a
                // template-linked policy, you must update the template
                // instead": a template-linked policy holds no content of its
                // own, so there is nothing here for a static definition to
                // replace, and accepting one would also be the forbidden change
                // of a policy's kind.
                if policy.template_link.is_some() {
                    return Err(invalid(crate::ops::InvalidInput::new(
                        crate::ops::paths::DEFINITION,
                        TEMPLATE_LINKED_IS_NOT_UPDATABLE,
                    )));
                }
                let scope = crate::state::cedar::parse_static_policy(&def.statement)
                    .map_err(invalid)?;

                // The elements the operation documents as unchangeable: "You
                // can't change these elements of a static policy: ... Changing
                // the effect of a static policy from permit or forbid. The
                // principal referenced by a static policy. The resource
                // referenced by a static policy." Each one that the replacement
                // would change is reported as its own `fieldList` entry, naming
                // the member that carries the change.
                let immutable = immutable_changes(policy, &scope);
                if !immutable.is_empty() {
                    return Err(refuse(immutable));
                }

                // The replacement is validated exactly as a create is: "If
                // policy validation is enabled in the policy store, then
                // updating a static policy causes Verified Permissions to
                // validate the policy against the schema in the policy store. If
                // the updated static policy doesn't pass validation, the
                // operation fails and the update isn't stored."
                if store.validation_mode == "STRICT" {
                    match store.schema.as_ref() {
                        None => {
                            return Err(invalid(crate::ops::InvalidInput::new(
                                crate::ops::paths::POLICY_STORE_ID,
                                "its validation mode is STRICT and it contains no schema, so \
                                 there is nothing to validate a policy against",
                            )));
                        }
                        Some(schema) => {
                            crate::state::cedar::validate_static_policy(
                                &schema.cedar_json,
                                &def.statement,
                            )
                            .map_err(refused)?;
                        }
                    }
                }

                // The store's Cedar-text budget, which a replacement can grow:
                // the bytes this policy already contributes come out and the
                // replacement's go in, so an update is held to the same bound
                // `CreatePolicy` holds a create to — the bound every later
                // authorization request parses within.
                let without_this_policy = store
                    .cedar_bytes()
                    .saturating_sub(policy.statement.as_ref().map_or(0, String::len));
                if without_this_policy + def.statement.len()
                    > crate::state::MAX_POLICY_BYTES_PER_STORE
                {
                    return Err(UpdatePolicyErrorModeled::ServiceQuotaExceededException(
                        ServiceQuotaExceededException {
                            message: format!(
                                "policy store {store_id} is at its limit of {} bytes of Cedar \
                                 text",
                                crate::state::MAX_POLICY_BYTES_PER_STORE
                            ),
                            resource_id: Some(store_id.clone()),
                            resource_type: crate::state::RESOURCE_TYPE_POLICY.to_string(),
                            ..Default::default()
                        },
                    )
                    .into());
                }

                Some((def.clone(), scope))
            }
        };

        // "If you specify a name that is already associated with another policy
        // in the policy store, you receive a ConflictException error" — another
        // policy: a request repeating this policy's own name changes nothing.
        if let Some(name) = input.name.as_deref() {
            if let Some(clash) = store
                .policies
                .values()
                .find(|p| p.name.as_deref() == Some(name) && p.policy_id != policy_id)
            {
                return Err(UpdatePolicyErrorModeled::ConflictException(crate::ops::conflict(
                    [crate::ops::resource_conflict(
                        &clash.policy_id,
                        crate::state::RESOURCE_TYPE_POLICY,
                    )],
                ))
                .into());
            }
        }

        let store = state
            .policy_stores
            .get_mut(&store_id)
            .ok_or_else(store_not_found)?;
        let policy = store
            .policies
            .get_mut(&policy_id)
            .ok_or_else(policy_not_found)?;
        if let Some((def, scope)) = replacement {
            policy.statement = Some(def.statement);
            // `description` is a member of the DEFINITION being replaced, not a
            // request member beside it: a replacement that carries no
            // description describes a policy that has none, exactly as a create
            // with no description does. (`name` IS its own request member, and
            // follows its own documented rule below.)
            policy.description = def.description;
            // The scope's mutable part: the actions the new text names. The
            // effect, the principal and the resource cannot differ from what is
            // stored — the check above refused a replacement that changed one —
            // so they are left as they are rather than written back.
            policy.actions = scope.actions;
        }
        // "If you don't include the name in an update request, the existing name
        // is unchanged. To remove a name, set it to an empty string ("")."
        if let Some(name) = input.name.clone() {
            policy.name = (!name.is_empty()).then_some(name);
        }
        policy.last_updated_date = crate::state::now();

        // Read the response back out of persisted state.
        let stored = store.policies.get(&policy_id).ok_or_else(unavailable)?;
        let output = UpdatePolicyOutput {
            policy_store_id: stored.policy_store_id.clone(),
            policy_id: stored.policy_id.clone(),
            policy_type: stored.policy_type.clone(),
            principal: stored
                .principal
                .as_ref()
                .map(super::create_policy::entity_identifier),
            resource: stored
                .resource
                .as_ref()
                .map(super::create_policy::entity_identifier),
            actions: (!stored.actions.is_empty()).then(|| {
                stored
                    .actions
                    .iter()
                    .map(super::create_policy::action_identifier)
                    .collect()
            }),
            created_date: stored.created_date,
            last_updated_date: stored.last_updated_date,
            effect: stored.effect.clone(),
        };
        crate::ops::audit(ctx, "UpdatePolicy", &output.policy_id, "updated");
        Ok(output)
    }
}

/// Why a template-linked policy cannot be updated through this operation, in the
/// operation's own words: "To change a template-linked policy, you must update
/// the template instead, using UpdatePolicyTemplate."
const TEMPLATE_LINKED_IS_NOT_UPDATABLE: &str =
    "You can't change the content of a template-linked policy; update the policy template \
     it is linked to instead.";

/// Which elements the replacement would change that an existing policy may not
/// change — one entry per element, each naming the member that carries the
/// change, so a client reading `fieldList` is told about every one of them.
///
/// The list is the operation's own: "You can't change these elements of a static
/// policy: Changing a policy from a static policy to a template-linked policy.
/// Changing the effect of a static policy from permit or forbid. The principal
/// referenced by a static policy. The resource referenced by a static policy."
/// An element the stored policy does not name is unchanged only by a replacement
/// that does not name it either, which is what comparing the parsed scopes says.
fn immutable_changes(
    stored: &crate::state::Policy,
    replacement: &crate::state::cedar::StaticScope,
) -> Vec<crate::ops::InvalidInput> {
    let mut changes = Vec::new();
    if stored.effect.as_deref() != Some(replacement.effect.as_str()) {
        changes.push(crate::ops::InvalidInput::new(
            crate::ops::paths::STATIC_STATEMENT,
            "You can't change the effect of an existing policy.",
        ));
    }
    if stored.principal != replacement.principal {
        changes.push(crate::ops::InvalidInput::new(
            crate::ops::paths::STATIC_STATEMENT,
            "You can't change the principal referenced in the scope of an existing policy.",
        ));
    }
    if stored.resource != replacement.resource {
        changes.push(crate::ops::InvalidInput::new(
            crate::ops::paths::STATIC_STATEMENT,
            "You can't change the resource referenced in the scope of an existing policy.",
        ));
    }
    changes
}
