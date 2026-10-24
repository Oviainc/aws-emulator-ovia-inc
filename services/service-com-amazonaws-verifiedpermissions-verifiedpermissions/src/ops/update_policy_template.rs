// SPDX-License-Identifier: CC0-1.0

//! @op UpdatePolicyTemplate
//!
//! REPLACES a policy template's Cedar text (and its name and description) in
//! [`crate::state`], so [`crate::ops::get_policy_template`] reports the new
//! statement — and so every policy linked to the template follows it, which is
//! what a template is for: "Changes you make to the policy template content are
//! immediately (within the constraints of eventual consistency) reflected in
//! authorization decisions that involve all template-linked policies
//! instantiated from this template."
//!
//! The replacement is held to the same two checks a create is held to — it must
//! be text the real Cedar TEMPLATE parser accepts
//! ([`crate::state::cedar::parse_policy_template`]), and in a `STRICT` store it
//! must validate against the store's schema ("Cedar performs validation on all
//! submitted new or updated static policies and policy templates") — plus the
//! elements `statement`'s own documentation makes immutable: "You can't change
//! the following elements: The effect (permit or forbid) of the policy template.
//! The principal referenced by the policy template. The resource referenced by
//! the policy template." Those are compared as Cedar SCOPES
//! ([`crate::state::cedar::template_scope`]), not as text, so reformatting or a
//! changed condition clause is accepted while a changed scope is refused.
//!
//! A refused request stores nothing: every check runs before anything is
//! written. The policies linked to the template follow it without any per-link
//! Cedar work — the replacement is parsed ONCE, and the only part of a linked
//! policy's reported scope that a permitted replacement can change is the
//! actions the template's scope names, which are the same for every link.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdatePolicyTemplate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Updates the specified policy template. You can update only the description and the some
//! elements of the policyBody. Changes you make to the policy template content are
//! immediately (within the constraints of eventual consistency) reflected in authorization
//! decisions that involve all template-linked policies instantiated from this template.
//! Verified Permissions is eventually consistent . It can take a few seconds for a new or
//! changed element to propagate through the service and be visible in the results of other
//! Verified Permissions operations.
//!
//! ## Input
//!
//! * `description` (PolicyTemplateDescription, len 0..150) — Specifies a new description to
//!   apply to the policy template.
//! * `name` (PolicyTemplateName, len 0..150, pattern ^[a-zA-Z0-9-/_]*$) — Specifies a name
//!   for the policy template that is unique among all policy templates within the policy
//!   store. You can use the name in place of the policy template ID in API operations that
//!   reference the policy template. The name must be prefixed with name/. If you don't
//!   include the name in an update request, the existing name is unchanged. To remove a name,
//!   set it to an empty string (""). If you specify a name that is already associated with
//!   another policy template in the policy store, you receive a ConflictException error.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store that contains the policy template that you want to
//!   update. To specify a policy store, use its ID or alias name. When using an alias name,
//!   prefix it with policy-store-alias/. For example: ID: PSEXAMPLEabcdefg111111 Alias name:
//!   policy-store-alias/example-policy-store To view aliases, use ListPolicyStoreAliases.
//! * `policyTemplateId` (PolicyTemplateId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$)
//!   **required** — Specifies the ID of the policy template that you want to update. You can
//!   use the policy template name in place of the policy template ID. When using a name,
//!   prefix it with name/. For example: ID: PTEXAMPLEabcdefg111111 Name:
//!   name/example-policy-template
//! * `statement` (PolicyStatement, len 1..) **required** — Specifies new statement content
//!   written in Cedar policy language to replace the current body of the policy template. You
//!   can change only the following elements of the policy body: The action referenced by the
//!   policy template. Any conditional clauses, such as when or unless clauses. You can't
//!   change the following elements: The effect (permit or forbid) of the policy template. The
//!   principal referenced by the policy template. The resource referenced by the policy
//!   template.
//!
//! ## Output
//!
//! * `createdDate` (TimestampFormat) **required** — The date and time that the policy
//!   template was originally created.
//! * `lastUpdatedDate` (TimestampFormat) **required** — The date and time that the policy
//!   template was most recently updated.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   The ID of the policy store that contains the updated policy template.
//! * `policyTemplateId` (PolicyTemplateId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$)
//!   **required** — The ID of the updated policy template.
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
// `UpdatePolicyTemplateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdatePolicyTemplateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#UpdatePolicyTemplate",
    extra_errors = ["InternalServerException", "ValidationException"]
)]
#[derive(Debug)]
pub enum UpdatePolicyTemplateErrorModeled {
    ConflictException(ConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_policy_template::UpdatePolicyTemplateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdatePolicyTemplateError = crate::scaffolded::Scaffolded<UpdatePolicyTemplateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdatePolicyTemplateErrorModeled> for UpdatePolicyTemplateError {
    fn from(modeled: UpdatePolicyTemplateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdatePolicyTemplate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdatePolicyTemplate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdatePolicyTemplate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdatePolicyTemplate<Eng>
{
    type Input = UpdatePolicyTemplateInput;
    type Output = UpdatePolicyTemplateOutput;
    type Error = UpdatePolicyTemplateError;

    const NAME: &'static str = "UpdatePolicyTemplate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_POLICY_TEMPLATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_POLICY_TEMPLATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store_not_found = || {
            UpdatePolicyTemplateErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };
        let template_not_found = || {
            UpdatePolicyTemplateErrorModeled::ResourceNotFoundException(
                crate::state::policy_template_reference(&input.policy_template_id).not_found(),
            )
        };
        let unavailable = || {
            UpdatePolicyTemplateErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        };
        let fault = |message: String| -> UpdatePolicyTemplateError {
            UpdatePolicyTemplateErrorModeled::InternalServerException(InternalServerException {
                message,
            })
            .into()
        };
        let refuse = |invalid: Vec<crate::ops::InvalidInput>| -> UpdatePolicyTemplateError {
            UpdatePolicyTemplateErrorModeled::ValidationException(
                crate::ops::validation_exception(invalid),
            )
            .into()
        };
        let invalid =
            |invalid: crate::ops::InvalidInput| -> UpdatePolicyTemplateError { refuse(vec![invalid]) };
        let refused = |refusal: crate::state::cedar::Refusal| -> UpdatePolicyTemplateError {
            match refusal {
                crate::state::cedar::Refusal::InvalidInput(field) => invalid(field),
                crate::state::cedar::Refusal::SchemaValidation(reasons) => {
                    UpdatePolicyTemplateErrorModeled::ValidationException(
                        crate::ops::schema_validation_exception(
                            crate::ops::paths::TEMPLATE_STATEMENT,
                            reasons,
                        ),
                    )
                    .into()
                }
                crate::state::cedar::Refusal::ServiceFault(message) => fault(message),
            }
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(unavailable)?;
        let mut state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(store_not_found)?;
        let store = state
            .policy_stores
            .get(&store_id)
            .ok_or_else(store_not_found)?;
        // `policyTemplateId` takes an id or a `name/<name>`, exactly as the
        // readers of this resource do.
        let template_id = store
            .resolve_policy_template_id(&input.policy_template_id)
            .ok_or_else(template_not_found)?;
        let template = store
            .policy_templates
            .get(&template_id)
            .ok_or_else(template_not_found)?;

        // The replacement must be Cedar TEMPLATE text (which also bounds its
        // size), and its scope is what the immutability rule is stated over.
        let replacement = crate::state::cedar::template_scope(&input.statement).map_err(invalid)?;
        // The stored template's own scope. Its text is text this service already
        // accepted, so text it can no longer read is service-side state and not
        // something the caller can fix from this request.
        let stored = crate::state::cedar::template_scope(&template.statement).map_err(|e| {
            fault(format!(
                "the stored policy template's scope could not be read, so a replacement \
                 cannot be checked against it: {e}"
            ))
        })?;
        let immutable = immutable_changes(&stored, &replacement);
        if !immutable.is_empty() {
            return Err(refuse(immutable));
        }

        // "Cedar performs validation on all submitted new or updated static
        // policies and policy templates" — the same rule `CreatePolicyTemplate`
        // applies to a create, including a STRICT store that holds no schema to
        // validate against.
        if store.validation_mode == "STRICT" {
            match store.schema.as_ref() {
                None => {
                    return Err(invalid(crate::ops::InvalidInput::new(
                        crate::ops::paths::POLICY_STORE_ID,
                        "its validation mode is STRICT and it contains no schema, so there \
                         is nothing to validate a policy template against",
                    )));
                }
                Some(schema) => {
                    crate::state::cedar::validate_policy_template(
                        &schema.cedar_json,
                        &input.statement,
                    )
                    .map_err(refused)?;
                }
            }
        }

        // The store's Cedar-text budget, which a replacement can grow: the bytes
        // this template already contributes come out and the replacement's go in,
        // so an update is held to the same bound a create is. This operation
        // models no `ServiceQuotaExceededException`, so a replacement the store
        // has no room for is refused as the invalid member it is.
        let without_this_template = store.cedar_bytes().saturating_sub(template.statement.len());
        if without_this_template + input.statement.len()
            > crate::state::MAX_POLICY_BYTES_PER_STORE
        {
            return Err(invalid(crate::ops::InvalidInput::new(
                crate::ops::paths::TEMPLATE_STATEMENT,
                format!(
                    "the policy store holds its limit of {} bytes of Cedar text, so this \
                     statement does not fit",
                    crate::state::MAX_POLICY_BYTES_PER_STORE
                ),
            )));
        }

        // "If you specify a name that is already associated with another policy
        // template in the policy store, you receive a ConflictException error" —
        // another template: a request repeating this template's own name changes
        // nothing.
        if let Some(name) = input.name.as_deref() {
            if let Some(clash) = store.policy_templates.values().find(|other| {
                other.name.as_deref() == Some(name) && other.policy_template_id != template_id
            }) {
                return Err(UpdatePolicyTemplateErrorModeled::ConflictException(
                    crate::ops::conflict([crate::ops::resource_conflict(
                        &clash.policy_template_id,
                        crate::state::RESOURCE_TYPE_POLICY_TEMPLATE,
                    )]),
                )
                .into());
            }
        }

        let store = state
            .policy_stores
            .get_mut(&store_id)
            .ok_or_else(store_not_found)?;
        let template = store
            .policy_templates
            .get_mut(&template_id)
            .ok_or_else(template_not_found)?;
        template.statement = input.statement.clone();
        // Only `name` carries a documented "unchanged unless included" rule ("If
        // you don't include the name in an update request, the existing name is
        // unchanged. To remove a name, set it to an empty string"), so
        // `description` follows the request, as it does on `UpdatePolicyStore`: a
        // call carrying none leaves the template without one.
        template.description = input.description.clone();
        if let Some(name) = input.name.clone() {
            template.name = (!name.is_empty()).then_some(name);
        }
        template.last_updated_date = crate::state::now();

        // Every policy linked to this template follows it, and the ONLY part of
        // such a policy's reported scope a replacement can change is the actions
        // the template's scope names: its effect, its principal and its resource
        // cannot differ from the stored template's (the check above refused a
        // replacement that changed one), and Cedar has no placeholder in the
        // action position, so one template gives every policy linked to it the
        // same actions. They therefore come from the replacement's scope, parsed
        // ONCE above — the work this loop does per linked policy is a clone of
        // that list, never another Cedar parse or link.
        //
        // A linked policy's own `lastUpdatedDate` is left alone: what this call
        // updated is the template, and a linked policy holds no content of its
        // own to update — it follows whatever the template now says.
        for policy in store.policies.values_mut() {
            if policy.policy_template_id() == Some(template_id.as_str()) {
                policy.actions = replacement.actions.clone();
            }
        }

        // Read the response back out of persisted state.
        let stored = store
            .policy_templates
            .get(&template_id)
            .ok_or_else(unavailable)?;
        let output = UpdatePolicyTemplateOutput {
            policy_store_id: stored.policy_store_id.clone(),
            policy_template_id: stored.policy_template_id.clone(),
            created_date: stored.created_date,
            last_updated_date: stored.last_updated_date,
        };
        crate::ops::audit(
            ctx,
            "UpdatePolicyTemplate",
            &output.policy_template_id,
            "updated",
        );
        Ok(output)
    }
}

/// Which elements the replacement would change that a policy template may not
/// change — one entry per element, each naming `statement`, the member that
/// carries the change, so a client reading `fieldList` is told about every one.
///
/// The list is `UpdatePolicyTemplateInput$statement`'s own: "You can't change
/// the following elements: The effect (permit or forbid) of the policy template.
/// The principal referenced by the policy template. The resource referenced by
/// the policy template." What is compared is the SCOPE Cedar reports for each
/// template ([`crate::state::cedar::template_scope`]), so a position holding a
/// placeholder compares equal to the same placeholder and a position naming an
/// entity only compares equal to the same entity. The elements the operation
/// does allow to change — "The action referenced by the policy template. Any
/// conditional clauses" — are deliberately not compared.
fn immutable_changes(
    stored: &crate::state::cedar::StaticScope,
    replacement: &crate::state::cedar::StaticScope,
) -> Vec<crate::ops::InvalidInput> {
    let mut changes = Vec::new();
    if stored.effect != replacement.effect {
        changes.push(crate::ops::InvalidInput::new(
            crate::ops::paths::TEMPLATE_STATEMENT,
            "You can't change the effect of an existing policy template.",
        ));
    }
    if stored.principal != replacement.principal {
        changes.push(crate::ops::InvalidInput::new(
            crate::ops::paths::TEMPLATE_STATEMENT,
            "You can't change the principal referenced in the scope of an existing policy \
             template.",
        ));
    }
    if stored.resource != replacement.resource {
        changes.push(crate::ops::InvalidInput::new(
            crate::ops::paths::TEMPLATE_STATEMENT,
            "You can't change the resource referenced in the scope of an existing policy \
             template.",
        ));
    }
    changes
}
