// SPDX-License-Identifier: CC0-1.0

//! @op GetPolicy
//!
//! `@readonly`: reports the persisted policy, including the Cedar `statement`
//! byte-identical to what `CreatePolicy` was given. It writes nothing.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetPolicy`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Retrieves information about the specified policy.
//!
//! ## Input
//!
//! * `policyId` (PolicyId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** — Specifies
//!   the ID of the policy you want information about. You can use the policy name in place of
//!   the policy ID. When using a name, prefix it with name/. For example: ID:
//!   SPEXAMPLEabcdefg111111 Name: name/example-policy
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store that contains the policy that you want information
//!   about. To specify a policy store, use its ID or alias name. When using an alias name,
//!   prefix it with policy-store-alias/. For example: ID: PSEXAMPLEabcdefg111111 Alias name:
//!   policy-store-alias/example-policy-store To view aliases, use ListPolicyStoreAliases.
//!
//! ## Output
//!
//! * `actions` (ActionIdentifierList) — The action that a policy permits or forbids. For
//!   example, {"actions": [{"actionId": "ViewPhoto", "actionType": "PhotoFlash::Action"},
//!   {"entityID": "SharePhoto", "entityType": "PhotoFlash::Action"}]}.
//! * `createdDate` (TimestampFormat) **required** — The date and time that the policy was
//!   originally created.
//! * `definition` (PolicyDefinitionDetail) **required** — The definition of the requested
//!   policy.
//! * `effect` (PolicyEffect, one of: Forbid | Permit) — The effect of the decision that a
//!   policy returns to an authorization request. For example, "effect": "Permit".
//! * `lastUpdatedDate` (TimestampFormat) **required** — The date and time that the policy was
//!   last updated.
//! * `name` (PolicyName, len 0..150, pattern ^[a-zA-Z0-9-/_]*$) — The name of the policy, if
//!   one was assigned when the policy was created or last updated.
//! * `policyId` (PolicyId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** — The unique
//!   ID of the policy that you want information about.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   The ID of the policy store that contains the policy that you want information about.
//! * `policyType` (PolicyType, one of: STATIC | TEMPLATE_LINKED) **required** — The type of
//!   the policy.
//! * `principal` (EntityIdentifier) — The principal specified in the policy's scope. This
//!   element isn't included in the response when Principal isn't present in the policy
//!   content.
//! * `resource` (EntityIdentifier) — The resource specified in the policy's scope. This
//!   element isn't included in the response when Resource isn't present in the policy
//!   content.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 404) — The request failed because it references a
//!   resource that doesn't exist.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetPolicyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetPolicyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#GetPolicy",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum GetPolicyErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_policy::GetPolicyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetPolicyError = crate::scaffolded::Scaffolded<GetPolicyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetPolicyErrorModeled> for GetPolicyError {
    fn from(modeled: GetPolicyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetPolicy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetPolicy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetPolicy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetPolicy<Eng>
{
    type Input = GetPolicyInput;
    type Output = GetPolicyOutput;
    type Error = GetPolicyError;

    const NAME: &'static str = "GetPolicy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_POLICY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_POLICY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let store_not_found = || {
            GetPolicyErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };
        let policy_not_found = || {
            GetPolicyErrorModeled::ResourceNotFoundException(
                crate::state::policy_reference(&input.policy_id).not_found(),
            )
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetPolicyErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(store_not_found)?;
        let policy = state
            .policy_stores
            .get(&store_id)
            .and_then(|store| {
                store
                    .resolve_policy_id(&input.policy_id)
                    .and_then(|policy_id| store.policies.get(&policy_id))
            })
            .ok_or_else(policy_not_found)?;

        // A `PolicyDefinitionDetail` must agree with `policyType`, so the arm is
        // chosen by what the stored policy HOLDS rather than assumed.
        let definition = policy_definition_detail(policy);

        Ok(GetPolicyOutput {
            policy_store_id: policy.policy_store_id.clone(),
            policy_id: policy.policy_id.clone(),
            policy_type: policy.policy_type.clone(),
            principal: policy
                .principal
                .as_ref()
                .map(super::create_policy::entity_identifier),
            resource: policy
                .resource
                .as_ref()
                .map(super::create_policy::entity_identifier),
            actions: (!policy.actions.is_empty()).then(|| {
                policy
                    .actions
                    .iter()
                    .map(super::create_policy::action_identifier)
                    .collect()
            }),
            definition,
            created_date: policy.created_date,
            last_updated_date: policy.last_updated_date,
            effect: policy.effect.clone(),
            name: policy.name.clone(),
        })
    }
}

/// A persisted policy's `PolicyDefinitionDetail`.
///
/// The union arm is chosen by what the policy HOLDS — its own Cedar text, or the
/// template it was instantiated from — so the detail always agrees with the
/// `policyType` reported beside it. A template-linked policy's detail carries
/// the `policyTemplateId` it follows and the entities it bound that template's
/// placeholders to, and no statement: the Cedar text belongs to the template,
/// which `GetPolicyTemplate` reports. Shared with `BatchGetPolicy`, which
/// reports the same union for the same stored policies.
pub fn policy_definition_detail(policy: &crate::state::Policy) -> PolicyDefinitionDetail {
    match policy.template_link.as_ref() {
        Some(link) => {
            PolicyDefinitionDetail::templateLinked(TemplateLinkedPolicyDefinitionDetail {
                policy_template_id: link.policy_template_id.clone(),
                principal: link.principal.as_ref().map(super::create_policy::entity_identifier),
                resource: link.resource.as_ref().map(super::create_policy::entity_identifier),
            })
        }
        None => PolicyDefinitionDetail::Static(StaticPolicyDefinitionDetail {
            description: policy.description.clone(),
            // A static policy always holds its text; `unwrap_or_default` is the
            // non-panicking reading of that invariant, never a fabricated value.
            statement: policy.statement.clone().unwrap_or_default(),
        }),
    }
}
