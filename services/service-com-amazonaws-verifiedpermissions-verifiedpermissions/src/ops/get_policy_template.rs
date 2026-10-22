// SPDX-License-Identifier: CC0-1.0

//! @op GetPolicyTemplate
//!
//! Reports one policy template out of the store that holds it: a pure read, so
//! it writes nothing. The template is addressed either by its id or by
//! `name/<policyTemplateName>`, which the model documents as interchangeable,
//! and every member comes out of persisted state — this is the reader that makes
//! `CreatePolicyTemplate`'s write observable.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetPolicyTemplate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Retrieve the details for the specified policy template in the specified policy store.
//!
//! ## Input
//!
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store that contains the policy template that you want
//!   information about. To specify a policy store, use its ID or alias name. When using an
//!   alias name, prefix it with policy-store-alias/. For example: ID: PSEXAMPLEabcdefg111111
//!   Alias name: policy-store-alias/example-policy-store To view aliases, use
//!   ListPolicyStoreAliases.
//! * `policyTemplateId` (PolicyTemplateId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$)
//!   **required** — Specifies the ID of the policy template that you want information about.
//!   You can use the policy template name in place of the policy template ID. When using a
//!   name, prefix it with name/. For example: ID: PTEXAMPLEabcdefg111111 Name:
//!   name/example-policy-template
//!
//! ## Output
//!
//! * `createdDate` (TimestampFormat) **required** — The date and time that the policy
//!   template was originally created.
//! * `description` (PolicyTemplateDescription, len 0..150) — The description of the policy
//!   template.
//! * `lastUpdatedDate` (TimestampFormat) **required** — The date and time that the policy
//!   template was most recently updated.
//! * `name` (PolicyTemplateName, len 0..150, pattern ^[a-zA-Z0-9-/_]*$) — The name of the
//!   policy template, if one was assigned when the policy template was created or last
//!   updated.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   The ID of the policy store that contains the policy template.
//! * `policyTemplateId` (PolicyTemplateId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$)
//!   **required** — The ID of the policy template.
//! * `statement` (PolicyStatement, len 1..) **required** — The content of the body of the
//!   policy template written in the Cedar policy language.
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
// `GetPolicyTemplateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetPolicyTemplateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#GetPolicyTemplate",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum GetPolicyTemplateErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_policy_template::GetPolicyTemplateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetPolicyTemplateError = crate::scaffolded::Scaffolded<GetPolicyTemplateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetPolicyTemplateErrorModeled> for GetPolicyTemplateError {
    fn from(modeled: GetPolicyTemplateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetPolicyTemplate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetPolicyTemplate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetPolicyTemplate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetPolicyTemplate<Eng>
{
    type Input = GetPolicyTemplateInput;
    type Output = GetPolicyTemplateOutput;
    type Error = GetPolicyTemplateError;

    const NAME: &'static str = "GetPolicyTemplate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_POLICY_TEMPLATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_POLICY_TEMPLATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let store_not_found = || {
            GetPolicyTemplateErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };
        let template_not_found = || {
            GetPolicyTemplateErrorModeled::ResourceNotFoundException(
                crate::state::policy_template_reference(&input.policy_template_id).not_found(),
            )
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetPolicyTemplateErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(store_not_found)?;
        let template = state
            .policy_stores
            .get(&store_id)
            .and_then(|store| {
                store
                    .resolve_policy_template_id(&input.policy_template_id)
                    .and_then(|template_id| store.policy_templates.get(&template_id))
            })
            .ok_or_else(template_not_found)?;

        Ok(GetPolicyTemplateOutput {
            policy_store_id: template.policy_store_id.clone(),
            policy_template_id: template.policy_template_id.clone(),
            description: template.description.clone(),
            statement: template.statement.clone(),
            created_date: template.created_date,
            last_updated_date: template.last_updated_date,
            name: template.name.clone(),
        })
    }
}
