// SPDX-License-Identifier: CC0-1.0

//! @op GetSchema
//!
//! Reports the one Cedar schema a policy store holds: the text `PutSchema`
//! stored, the namespaces read out of it and its timestamps. A store that has
//! never been given a schema holds the EMPTY schema document (`{}`, no
//! namespaces, dated at the store's creation) — the same document a caller
//! returns a store to by putting `{}` — so this call answers for every policy
//! store that exists, and 404s only when the store itself does not.
//!
//! `@readonly`: it reads [`crate::state`] and writes nothing.
//!
//! Retrieve the details for the specified schema in the specified policy store.
//!
//! ## Input
//!
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store that contains the schema. To specify a policy
//!   store, use its ID or alias name. When using an alias name, prefix it with
//!   policy-store-alias/. For example: ID: PSEXAMPLEabcdefg111111 Alias name:
//!   policy-store-alias/example-policy-store To view aliases, use ListPolicyStoreAliases.
//!
//! ## Output
//!
//! * `createdDate` (TimestampFormat) **required** — The date and time that the schema was
//!   originally created.
//! * `lastUpdatedDate` (TimestampFormat) **required** — The date and time that the schema was
//!   most recently updated.
//! * `namespaces` (NamespaceList) — The namespaces of the entities referenced by this schema.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   The ID of the policy store that contains the schema.
//! * `schema` (SchemaJson, len 1..) **required** — The body of the schema, written in Cedar
//!   schema JSON.
//! * `definition` (SchemaDefinition) — the same schema document in the union form
//!   `PutSchema` takes it in (`cedarJson`). NOT declared by the checked-in
//!   `GetSchemaOutput`: the recorded response carries it beside `schema`, and the
//!   recording is what the wire is, so it is reported from the same stored
//!   document rather than left off for a client whose SDK reads the union arm.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 404) — no such policy store
//!   (`resourceType` `POLICY_STORE`).
//! * `InternalServerException` (HTTP 500) — the state store is unreachable.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetSchemaError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `InternalServerException` is in this service's
// common error set, so it is named through `extra_errors` the way the other
// hand-authored handlers do: it is what answers an unreachable state store.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#GetSchema",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum GetSchemaErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_schema::GetSchemaError`).
pub type GetSchemaError = crate::scaffolded::Scaffolded<GetSchemaErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<GetSchemaErrorModeled> for GetSchemaError {
    fn from(modeled: GetSchemaErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetSchema` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetSchema<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetSchema<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetSchema<Eng>
{
    type Input = GetSchemaInput;
    type Output = GetSchemaOutput;
    type Error = GetSchemaError;

    const NAME: &'static str = "GetSchema";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_SCHEMA_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_SCHEMA_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Reporting only: nothing here writes.
        let _ = ctx;
        let store_not_found = || {
            GetSchemaErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetSchemaErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(store_not_found)?;
        let store = state
            .policy_stores
            .get(&store_id)
            .ok_or_else(store_not_found)?;

        // A store that exists always has a schema document to report: the one
        // `PutSchema` wrote, or the EMPTY document until it does. "No schema" is
        // a STATE of that one document, not a missing resource — which is why
        // this is a 200 and not a `ResourceNotFoundException` naming `SCHEMA`.
        // Nothing is written to answer: see
        // [`crate::state::PolicyStore::schema_document`].
        let schema = store.schema_document();

        Ok(GetSchemaOutput {
            policy_store_id: store_id.clone(),
            schema: schema.cedar_json.clone(),
            created_date: schema.created_date,
            last_updated_date: schema.last_updated_date,
            // Modelled optional here (unlike on `PutSchema`), but a schema
            // document always knows its namespaces, so they are always reported.
            namespaces: Some(schema.namespaces.clone()),
            // The same one document, in the union form the writer takes it in.
            definition: SchemaDefinition::cedarJson(schema.cedar_json.clone()),
        })
    }
}
