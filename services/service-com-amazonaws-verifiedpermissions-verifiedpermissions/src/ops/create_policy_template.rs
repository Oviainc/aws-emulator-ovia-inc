// SPDX-License-Identifier: CC0-1.0

//! @op CreatePolicyTemplate
//!
//! Stores a Cedar policy TEMPLATE in a policy store. The caller's Cedar text is
//! kept verbatim and parsed by the real Cedar TEMPLATE parser
//! ([`crate::state::cedar::parse_policy_template`]) — `?principal`/`?resource`
//! are slots, which the static-policy parser refuses by design — so a store
//! never holds a template that is not valid Cedar, and every response field is
//! read back out of what was persisted.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreatePolicyTemplate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Creates a policy template. A template can use placeholders for the principal and
//! resource. A template must be instantiated into a policy by associating it with specific
//! principals and resources to use for the placeholders. That instantiated policy can then
//! be considered in authorization decisions. The instantiated policy works identically to
//! any other policy, except that it is dynamically linked to the template. If the template
//! changes, then any policies that are linked to that template are immediately updated as
//! well. Verified Permissions is eventually consistent . It can take a few seconds for a
//! new or changed element to propagate through the service and be visible in the results of
//! other Verified Permissions operations.
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
//! * `description` (PolicyTemplateDescription, len 0..150) — Specifies a description for the
//!   policy template.
//! * `name` (PolicyTemplateName, len 0..150, pattern ^[a-zA-Z0-9-/_]*$) — Specifies a name
//!   for the policy template that is unique among all policy templates within the policy
//!   store. You can use the name in place of the policy template ID in API operations that
//!   reference the policy template. The name must be prefixed with name/. If you specify a
//!   name that is already associated with another policy template in the policy store, you
//!   receive a ConflictException error.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   The ID of the policy store in which to create the policy template. To specify a policy
//!   store, use its ID or alias name. When using an alias name, prefix it with
//!   policy-store-alias/. For example: ID: PSEXAMPLEabcdefg111111 Alias name:
//!   policy-store-alias/example-policy-store To view aliases, use ListPolicyStoreAliases.
//! * `statement` (PolicyStatement, len 1..) **required** — Specifies the content that you
//!   want to use for the new policy template, written in the Cedar policy language.
//!
//! ## Output
//!
//! * `createdDate` (TimestampFormat) **required** — The date and time the policy template was
//!   originally created.
//! * `lastUpdatedDate` (TimestampFormat) **required** — The date and time the policy template
//!   was most recently updated.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   The ID of the policy store that contains the policy template.
//! * `policyTemplateId` (PolicyTemplateId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$)
//!   **required** — The unique ID of the new policy template.
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
// `CreatePolicyTemplateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreatePolicyTemplateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#CreatePolicyTemplate",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum CreatePolicyTemplateErrorModeled {
    ConflictException(ConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_policy_template::CreatePolicyTemplateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreatePolicyTemplateError = crate::scaffolded::Scaffolded<CreatePolicyTemplateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreatePolicyTemplateErrorModeled> for CreatePolicyTemplateError {
    fn from(modeled: CreatePolicyTemplateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreatePolicyTemplate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreatePolicyTemplate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreatePolicyTemplate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreatePolicyTemplate<Eng>
{
    type Input = CreatePolicyTemplateInput;
    type Output = CreatePolicyTemplateOutput;
    type Error = CreatePolicyTemplateError;

    const NAME: &'static str = "CreatePolicyTemplate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_POLICY_TEMPLATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_POLICY_TEMPLATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let not_found = || {
            CreatePolicyTemplateErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };
        let unavailable = || {
            CreatePolicyTemplateErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(unavailable)?;
        let mut state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;

        // `@idempotencyToken` replay: the same token answers with the template the
        // first call created; the same token with different parameters conflicts.
        let parameters = crate::state::idempotency_parameters(&[
            ("store", Some(store_id.as_str())),
            ("name", input.name.as_deref()),
            ("statement", Some(input.statement.as_str())),
            ("description", input.description.as_deref()),
        ]);
        if let Some(token) = input.client_token.as_deref() {
            if let Some(record) = state.idempotency_replay("CreatePolicyTemplate", token) {
                if record.parameters != parameters {
                    // Fixed message; the template the token already created is
                    // reported in `resources`.
                    return Err(CreatePolicyTemplateErrorModeled::ConflictException(
                        crate::ops::conflict([crate::ops::resource_conflict(
                            &record.resource_id,
                            crate::state::RESOURCE_TYPE_POLICY_TEMPLATE,
                        )]),
                    )
                    .into());
                }
                if let Some(template) = state
                    .policy_stores
                    .get(&store_id)
                    .and_then(|store| store.policy_templates.get(&record.resource_id))
                {
                    return Ok(output_for(template));
                }
            }
        }

        // `resolve_store_id` answers only for a store that is present, so this is
        // a lookup of a key it just confirmed — but it is still resolved through
        // the error channel rather than an `expect`, because the key came from the
        // request and a handler must not be able to panic on caller input.
        let store = state.policy_stores.get(&store_id).ok_or_else(not_found)?;

        // The statement is Cedar TEMPLATE text, so the real Cedar template parser
        // decides whether it is a template at all. One it refuses is refused here,
        // BEFORE the name-conflict and quota checks: a malformed request is a 400
        // whatever else the store already holds. (The `clientToken` replay above
        // comes first, as it does in `CreatePolicy`: a token reused with different
        // parameters is a conflict however well-formed this statement is.)
        crate::state::cedar::parse_policy_template(&input.statement)
            .map_err(crate::scaffolded::Scaffolded::validation)?;

        // `ValidationSettings` documents this exactly: "STRICT — Requires a schema
        // to be present in the policy store. Cedar performs validation on all
        // submitted new or updated static policies and policy templates. Any that
        // fail validation are rejected and Cedar doesn't store them in the policy
        // store", and "If Mode=STRICT and the policy store doesn't contain a
        // schema, Verified Permissions rejects all static policies and policy
        // templates because there is no schema to validate against." A template is
        // one of the two things that validation covers, so it follows the same
        // rule `CreatePolicy` applies to a static policy.
        if store.validation_mode == "STRICT" {
            match store.schema.as_ref() {
                None => {
                    return Err(crate::scaffolded::Scaffolded::validation(format!(
                        "policy store {store_id} has validation mode STRICT but contains no \
                         schema, so the policy template cannot be validated"
                    )));
                }
                Some(schema) => {
                    crate::state::cedar::validate_policy_template(
                        &schema.cedar_json,
                        &input.statement,
                    )
                    .map_err(crate::scaffolded::Scaffolded::validation)?;
                }
            }
        }

        // "Specifies a name for the policy template that is unique among all
        // policy templates within the policy store. ... If you specify a name that
        // is already associated with another policy template in the policy store,
        // you receive a ConflictException error." Checked BEFORE the quota, so a
        // name that already exists is the documented conflict whether or not the
        // store is also at its bound.
        if let Some(name) = input.name.as_deref() {
            if let Some(clash) = store
                .policy_templates
                .values()
                .find(|template| template.name.as_deref() == Some(name))
            {
                return Err(CreatePolicyTemplateErrorModeled::ConflictException(
                    crate::ops::conflict([crate::ops::resource_conflict(
                        &clash.policy_template_id,
                        crate::state::RESOURCE_TYPE_POLICY_TEMPLATE,
                    )]),
                )
                .into());
            }
        }

        if store.policy_templates.len() >= crate::state::MAX_POLICY_TEMPLATES_PER_STORE
            || store.cedar_bytes() + input.statement.len()
                > crate::state::MAX_POLICY_BYTES_PER_STORE
        {
            return Err(
                CreatePolicyTemplateErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: format!(
                            "policy store {store_id} is at its limit of {} policy templates / \
                             {} bytes of Cedar text",
                            crate::state::MAX_POLICY_TEMPLATES_PER_STORE,
                            crate::state::MAX_POLICY_BYTES_PER_STORE
                        ),
                        // The quota is on the store's policy templates as a whole,
                        // so there is no one template id to name here.
                        resource_type: "POLICY_TEMPLATE".to_string(),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        let store = state
            .policy_stores
            .get_mut(&store_id)
            .ok_or_else(not_found)?;
        let policy_template_id = store.mint_policy_template_id();
        let created = crate::state::now();
        let template = crate::state::PolicyTemplate {
            policy_template_id: policy_template_id.clone(),
            policy_store_id: store_id.clone(),
            name: input.name.clone(),
            description: input.description.clone(),
            statement: input.statement.clone(),
            created_date: created,
            last_updated_date: created,
        };
        store
            .policy_templates
            .insert(policy_template_id.clone(), template);
        if let Some(token) = input.client_token.as_deref() {
            state.record_idempotency(
                "CreatePolicyTemplate",
                token,
                &policy_template_id,
                parameters,
            );
        }

        // Read the response back out of persisted state.
        let stored = state
            .policy_stores
            .get(&store_id)
            .and_then(|store| store.policy_templates.get(&policy_template_id))
            .ok_or_else(unavailable)?;
        let output = output_for(stored);
        crate::ops::audit(
            ctx,
            "CreatePolicyTemplate",
            &output.policy_template_id,
            "created",
        );
        Ok(output)
    }
}

/// Render a persisted policy template as this operation's output. Every member is
/// read out of state, so a value that was never stored cannot appear on the wire.
fn output_for(template: &crate::state::PolicyTemplate) -> CreatePolicyTemplateOutput {
    CreatePolicyTemplateOutput {
        policy_store_id: template.policy_store_id.clone(),
        policy_template_id: template.policy_template_id.clone(),
        created_date: template.created_date,
        last_updated_date: template.last_updated_date,
    }
}
