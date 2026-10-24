// SPDX-License-Identifier: CC0-1.0

//! @op DeletePolicyTemplate
//!
//! Removes one policy template from the store that holds it, and with it every
//! policy that was instantiated from that template — the cascade the operation's
//! own documentation states. The template is addressed either by its id or by
//! `name/<policyTemplateName>`, and the store either by its id or by its
//! `policy-store-alias/<name>` alias, which the model documents as
//! interchangeable in both cases; a reference that resolves to nothing is the
//! modelled `ResourceNotFoundException`, so a repeat delete is a 404 rather than
//! a silent success.
//!
//! "Immediately removed from all future API responses" is what this handler
//! implements: a linked policy is gone from state when this call returns, so a
//! following `GetPolicy` answers the modelled 404 `ResourceNotFoundException`,
//! `ListPolicies`/`BatchGetPolicy` no longer report it and `IsAuthorized` no
//! longer evaluates the grant it carried.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeletePolicyTemplate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Deletes the specified policy template from the policy store. This operation also deletes
//! any policies that were created from the specified policy template. Those policies are
//! immediately removed from all future API responses, and are asynchronously deleted from
//! the policy store.
//!
//! ## Input
//!
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store that contains the policy template that you want to
//!   delete. To specify a policy store, use its ID or alias name. When using an alias name,
//!   prefix it with policy-store-alias/. For example: ID: PSEXAMPLEabcdefg111111 Alias name:
//!   policy-store-alias/example-policy-store To view aliases, use ListPolicyStoreAliases.
//! * `policyTemplateId` (PolicyTemplateId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$)
//!   **required** — Specifies the ID of the policy template that you want to delete. You can
//!   use the policy template name in place of the policy template ID. When using a name,
//!   prefix it with name/. For example: ID: PTEXAMPLEabcdefg111111 Name:
//!   name/example-policy-template
//!
//! ## Errors
//!
//! * `ConflictException` (HTTP 409) — The request failed because another request to modify a
//!   resource occurred at the same time.
//! * `ResourceNotFoundException` (HTTP 404) — The request failed because it references a
//!   resource that doesn't exist.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeletePolicyTemplateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeletePolicyTemplateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#DeletePolicyTemplate",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum DeletePolicyTemplateErrorModeled {
    ConflictException(ConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_policy_template::DeletePolicyTemplateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeletePolicyTemplateError = crate::scaffolded::Scaffolded<DeletePolicyTemplateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeletePolicyTemplateErrorModeled> for DeletePolicyTemplateError {
    fn from(modeled: DeletePolicyTemplateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeletePolicyTemplate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeletePolicyTemplate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeletePolicyTemplate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeletePolicyTemplate<Eng>
{
    type Input = DeletePolicyTemplateInput;
    type Output = DeletePolicyTemplateOutput;
    type Error = DeletePolicyTemplateError;

    const NAME: &'static str = "DeletePolicyTemplate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_POLICY_TEMPLATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_POLICY_TEMPLATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    /// Remove the named template and CASCADE onto the policies instantiated from
    /// it.
    ///
    /// The cascade is what this operation documents: "This operation also deletes
    /// any policies that were created from the specified policy template. Those
    /// policies are immediately removed from all future API responses, and are
    /// asynchronously deleted from the policy store." *Immediately removed from
    /// all future API responses* is the observable half, so the linked policies
    /// leave `policies` in the same write as the template: nothing a later reader
    /// of this store can reach keeps a link to a template that is gone. No
    /// deferred step is left for the clock to make due — the asynchronous half is
    /// the real service reclaiming storage behind an API surface that already
    /// reports those policies as absent.
    ///
    /// A template-linked policy holds no Cedar text of its own
    /// ([`crate::state::Policy::statement`] is `None` for it), so the cascade is
    /// not a convenience: a link whose template was removed could not be
    /// evaluated, reported or re-resolved at all.
    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store_not_found = || {
            DeletePolicyTemplateErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };
        let template_not_found = || {
            DeletePolicyTemplateErrorModeled::ResourceNotFoundException(
                crate::state::policy_template_reference(&input.policy_template_id).not_found(),
            )
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeletePolicyTemplateErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let mut state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(store_not_found)?;
        let store = state
            .policy_stores
            .get_mut(&store_id)
            .ok_or_else(store_not_found)?;
        // `policyTemplateId` takes a template id or `name/<policyTemplateName>`,
        // and only a template that is PRESENT resolves — so a repeat delete is
        // the modelled 404 this operation declares rather than a silent success.
        let template_id = store
            .resolve_policy_template_id(&input.policy_template_id)
            .ok_or_else(template_not_found)?;

        store.policy_templates.remove(&template_id);
        // The documented cascade: every policy whose definition is
        // template-linked TO THIS TEMPLATE goes with it. A policy is matched by
        // the link it persists, not by its text — a template-linked policy holds
        // none of its own — so a static policy that merely mentions the same
        // entities is untouched.
        let linked: Vec<String> = store
            .policies
            .values()
            .filter(|policy| policy.policy_template_id() == Some(template_id.as_str()))
            .map(|policy| policy.policy_id.clone())
            .collect();
        for policy_id in &linked {
            store.policies.remove(policy_id);
            crate::ops::audit(
                ctx,
                "DeletePolicyTemplate",
                policy_id,
                "deleted with its policy template",
            );
        }
        crate::ops::audit(ctx, "DeletePolicyTemplate", &template_id, "deleted");

        Ok(DeletePolicyTemplateOutput {})
    }
}
