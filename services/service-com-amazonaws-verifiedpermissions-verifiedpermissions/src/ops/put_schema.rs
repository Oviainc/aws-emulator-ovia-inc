// SPDX-License-Identifier: CC0-1.0

//! @op PutSchema
//!
//! Creates or updates the ONE Cedar schema a policy store holds: the schema text
//! is parsed by the real Cedar schema parser, persisted on the store, and the
//! namespaces that parse found are reported back. A store's schema is what a
//! later `CreatePolicy` validates against in `STRICT` mode and what types an
//! `IsAuthorized` request's entities and context.
//!
//! Creates or updates the policy schema in the specified policy store. The schema is used
//! to validate any Cedar policies and policy templates submitted to the policy store. Any
//! changes to the schema validate only policies and templates submitted after the schema
//! change. Existing policies and templates are not re-evaluated against the changed schema.
//!
//! The schema document is persisted in [`crate::state`] in the form a read
//! reports it ([`crate::state::canonical_schema_json`]) and its `namespaces` are
//! read off the schema the Cedar parser produced, so `GetSchema` reports what was
//! stored rather than anything re-derived from the request.
//!
//! ## Input
//!
//! * `definition` (SchemaDefinition) **required** — Specifies the definition of the schema to
//!   be stored. The schema definition must be written in Cedar schema JSON.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store in which to place the schema. To specify a policy
//!   store, use its ID or alias name. When using an alias name, prefix it with
//!   policy-store-alias/. For example: ID: PSEXAMPLEabcdefg111111 Alias name:
//!   policy-store-alias/example-policy-store To view aliases, use ListPolicyStoreAliases.
//!
//! ## Output
//!
//! * `createdDate` (TimestampFormat) **required** — The date and time that the schema was
//!   originally created.
//! * `lastUpdatedDate` (TimestampFormat) **required** — The date and time that the schema was
//!   last updated.
//! * `namespaces` (NamespaceList) **required** — Identifies the namespaces of the entities
//!   referenced by this schema.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   The unique ID of the policy store that contains the schema.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 404) — no such policy store.
//! * `ServiceQuotaExceededException` (HTTP 402) — the schema is larger than
//!   [`crate::state::cedar::MAX_SCHEMA_BYTES`].
//! * `ValidationException` (HTTP 400) — the Cedar parser refuses the schema.
//! * `InternalServerException` (HTTP 500) — the state store is unreachable.
//!
//! `ConflictException` (HTTP 409) is modeled for a concurrent modification of the
//! same store; this emulator serialises every request through one state lock, so
//! no request can observe that race and none is fabricated.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `PutSchemaError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors, plus `InternalServerException` from the
// service's common error set for an unreachable state store.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#PutSchema",
    extra_errors = ["InternalServerException", "ValidationException"]
)]
#[derive(Debug)]
pub enum PutSchemaErrorModeled {
    ConflictException(ConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_schema::PutSchemaError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutSchemaError = crate::scaffolded::Scaffolded<PutSchemaErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutSchemaErrorModeled> for PutSchemaError {
    fn from(modeled: PutSchemaErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutSchema` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutSchema<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutSchema<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutSchema<Eng>
{
    type Input = PutSchemaInput;
    type Output = PutSchemaOutput;
    type Error = PutSchemaError;

    const NAME: &'static str = "PutSchema";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_SCHEMA_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_SCHEMA_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let not_found = || {
            PutSchemaErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };
        let unavailable = || {
            PutSchemaErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        };

        let SchemaDefinition::cedarJson(definition) = &input.definition;

        let state = crate::state::service_state(&*self.engine).ok_or_else(unavailable)?;
        let mut state = state.lock();
        // The store is resolved FIRST: a schema is placed in a policy store, so a
        // reference that names no store is a missing resource whatever else the
        // request carries.
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;

        // How much schema text one store may hold is bounded, and a size limit is
        // what the modelled `ServiceQuotaExceededException` reports. This is the
        // ONE place that owns [`crate::state::cedar::MAX_SCHEMA_BYTES`]: it is the
        // only way schema text enters the service, so everything that re-parses a
        // stored schema is parsing text this check already bounded.
        if definition.len() > crate::state::cedar::MAX_SCHEMA_BYTES {
            return Err(PutSchemaErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: format!(
                        "definition.cedarJson is {} bytes, more than the {} bytes a policy \
                         store's schema may hold",
                        definition.len(),
                        crate::state::cedar::MAX_SCHEMA_BYTES
                    ),
                    resource_id: Some(store_id.clone()),
                    resource_type: crate::state::RESOURCE_TYPE_SCHEMA.to_string(),
                    // `serviceCode`/`quotaCode` name a quota registered with the
                    // Service Quotas service. This bound is the emulator's own,
                    // not a registered quota, so there is no code to report and
                    // both optional members are left absent.
                    ..Default::default()
                },
            )
            .into());
        }

        // The namespaces are a property of the schema text, so they come from the
        // real Cedar schema parser; text it refuses is refused here too, with its
        // own diagnostics. `{}` is a well-formed schema that declares nothing,
        // which is how the model documents clearing a store's definitions ("To
        // delete the schema, run PutSchema with `{}` for this parameter"): it is
        // stored, and reports no namespaces.
        let parsed = crate::state::cedar::parse_schema(definition).map_err(|e| {
            // The schema text is one request member, so the refusal names it in
            // `fieldList` rather than only describing it in prose.
            PutSchemaErrorModeled::ValidationException(crate::ops::validation_exception([
                crate::ops::InvalidInput::new(crate::ops::paths::SCHEMA_CEDAR_JSON, e),
            ]))
        })?;

        let store = state
            .policy_stores
            .get_mut(&store_id)
            .ok_or_else(not_found)?;

        let now = crate::state::now();
        // "The date and time that the schema was originally created": putting a
        // schema over an existing one UPDATES it, so the creation stamp stays the
        // first put's and only `lastUpdatedDate` moves.
        let created_date = store
            .schema
            .as_ref()
            .map_or(now, |existing| existing.created_date);
        store.schema = Some(crate::state::StoredSchema {
            // The document, not the caller's spelling of it: what a read reports
            // is the submitted JSON with its insignificant whitespace gone and
            // nothing else about it changed.
            cedar_json: crate::state::canonical_schema_json(definition),
            namespaces: parsed.namespaces,
            created_date,
            last_updated_date: now,
        });
        // The store itself changed, so its own `lastUpdatedDate` moves too. What
        // the store already holds is NOT re-validated: "Any changes to the schema
        // validate only policies and templates submitted after the schema change.
        // Existing policies and templates are not re-evaluated against the
        // changed schema."
        store.last_updated_date = now;

        // Read the response back out of what was just persisted, so a value that
        // did not reach state cannot appear on the wire.
        let stored = state
            .policy_stores
            .get(&store_id)
            .and_then(|store| store.schema.as_ref())
            .ok_or_else(unavailable)?;
        let output = PutSchemaOutput {
            policy_store_id: store_id.clone(),
            namespaces: stored.namespaces.clone(),
            created_date: stored.created_date,
            last_updated_date: stored.last_updated_date,
        };
        crate::ops::audit(ctx, "PutSchema", &output.policy_store_id, "schema stored");
        Ok(output)
    }
}
