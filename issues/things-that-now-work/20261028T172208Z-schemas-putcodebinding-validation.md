Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3ca5cccf5768308b81885b90d5cd9b0626890960

# PutCodeBinding mutates a schema in the AWS-owned registry, the one write that skips the crate's single write resolution

_Filed: 2026-10-28T17:22:08Z_
_Operation: PutCodeBinding | Class: validation_

> _Note: this issue was identified against the code base at commit `3ca5cccf5768308b81885b90d5cd9b0626890960`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other write in this crate that names a registry goes through `ComAmazonawsSchemasSchemasState::writable_registry`, which refuses a service-owned registry with 403 `ForbiddenException` before anything changes. `PutCodeBinding` is the only write that does not: it resolves through `ComAmazonawsSchemasSchemasState::schema_at_version_mut` — the READ resolution handed back under a mutable borrow — and then inserts a `CodeBinding` into `schema.code_bindings` of whatever schema it resolved, including a schema of the seeded service-owned `aws.events` registry (`crate::state::AWS_MANAGED_REGISTRY`).

So for one and the same AWS-owned schema this service answers two different things about who may change it:

* `UpdateSchema`, `DeleteSchema`, `DeleteSchemaVersion`, `CreateSchema`, `UpdateRegistry`, `DeleteRegistry`, `PutResourcePolicy`, `DeleteResourcePolicy` and `UntagResource` all answer 403 with the service's sentence "Registry aws.events is an AWS-managed registry; its contents are owned by AWS and cannot be created, updated or deleted.";
* `PutCodeBinding` answers 202 and persists a record on that schema, which a later `DescribeCodeBinding` then reports — so the write is real and observable, not a no-op.

The crate states the rule this breaks in two places, and `schema_at_version_mut`'s own doc never mentions ownership at all, so nothing in the crate records a deliberate carve-out for code bindings: the path simply never takes the second step. The state module's shared refusal helper is even worded for exactly this case ("a service-owned registry **or one of its schemas**"), which is the resource `PutCodeBinding` writes to.

## Evidence

The rule, stated on the one resolution every registry-naming write is supposed to make — services/service-com-amazonaws-schemas-schemas/src/state.rs:193-198:
```
    /// Every write this service serves that names a registry — to the registry itself,
    /// to a schema or schema version in it, or to its resource-based policy — takes the
    /// same two steps before it changes anything: the store must hold a registry of
    /// that name, and what is in a service-owned registry is AWS's, so a caller may
    /// read it but not write to it. Resolving and refusing together is what keeps the
    /// second step from being forgotten by a handler that remembers the first.
```
and the sentence that refusal is worded with — state.rs:3789-3796:
```
/// Refuse a caller's write to a service-owned registry or one of its schemas: AWS
/// owns that content, so a caller may read it but never change it.
pub fn managed_registry_is_read_only(registry: &str) -> ForbiddenException {
    forbidden(format!(
        "Registry {registry} is an AWS-managed registry; its contents are owned by AWS \
         and cannot be created, updated or deleted."
    ))
}
```

The write that skips it — services/service-com-amazonaws-schemas-schemas/src/ops/put_code_binding.rs:196-242:
```
        let (schema, version) = state
            .schema_at_version_mut(
                &input.registry_name,
                &input.schema_name,
                input.schema_version.as_deref(),
            )
            .map_err(|refusal| {
                PutCodeBindingError::from(PutCodeBindingErrorModeled::NotFoundException(
                    refusal.not_found(),
                ))
            })?;
...
            schema.code_bindings.insert(
                key.clone(),
                crate::state::CodeBinding {
                    language: language.to_string(),
                    schema_version: version,
                    created_date: now,
                    generation_due: now + crate::state::CODE_GENERATION_DURATION,
                },
            );
```
`schema_at_version_mut` is the read resolution under a mutable borrow and asks nothing about ownership — state.rs:296-311:
```
    pub fn schema_at_version_mut(
        &mut self,
        registry_name: &str,
        schema_name: &str,
        requested: Option<&str>,
    ) -> Result<(&mut Schema, u64), ReadResolutionError> {
        let version = self
            .readable_schema_version(registry_name, schema_name, requested)?
```
and `readable_registry`, the first step it builds on, says so explicitly — state.rs:224-225: "Unlike [`Self::writable_registry`] it asks nothing about who OWNS the registry — what AWS owns, a caller may read."

The sibling write to a schema in a registry, for contrast — services/service-com-amazonaws-schemas-schemas/src/ops/delete_schema.rs:182-195:
```
        let registry = state
            .writable_registry(&input.registry_name)
            .map_err(|refusal| match refusal {
                ...
                crate::state::RegistryWriteError::ServiceOwned(registry) => {
                    DeleteSchemaError::from(DeleteSchemaErrorModeled::ForbiddenException(
                        crate::state::managed_registry_is_read_only(&registry),
                    ))
                }
            })?;
```

The schemas this reaches are really in the store: `aws_managed_registries()` seeds `aws.events` with `scope: RegistryScope::Aws` and a `Schema` per catalogue entry (state.rs:2674-2732), each with `code_bindings: ::std::collections::BTreeMap::new()`.

Test coverage: none. tests/all/registry_write_resolution.rs is the table of "the ONE resolution every write that names a registry makes" and enumerates exactly eight rows — `CreateSchema`, `UpdateSchema`, `DeleteSchema`, `DeleteSchemaVersion`, `UpdateRegistry`, `DeleteRegistry`, `PutResourcePolicy`, `DeleteResourcePolicy` (registry_write_resolution.rs:74-85) — with no `PutCodeBinding` row. The two code-binding suites only ever use a caller-created registry: tests/all/code_binding_lifecycle.rs:29 `const REGISTRY: &str = "code-binding-registry";` created at line 139, and tests/all/code_bindings.rs drives only `Language` refusals; grepping both for `aws.events`/`AWS_MANAGED` matches nothing.

## Steps to Reproduce

1. GET /v1/registries/name/aws.events/schemas -> 200, listing the seeded service-owned catalogue; take a schema name from it (e.g. `aws.a4b@RoomStateChange`, seeded via state.rs:1790).
2. PUT /v1/registries/name/aws.events/schemas/name/aws.a4b@RoomStateChange with {"Content":"{}","Type":"OpenApi3"} (UpdateSchema) -> 403 ForbiddenException, "Registry aws.events is an AWS-managed registry; its contents are owned by AWS and cannot be created, updated or deleted." (update_schema.rs:190-203 via writable_registry).
3. POST /v1/registries/name/aws.events/schemas/name/aws.a4b@RoomStateChange/language/Java8 (PutCodeBinding) -> 202, and a CodeBinding is inserted into that AWS-owned schema's `code_bindings` map (put_code_binding.rs:233-241) — no ownership step was taken.
4. GET /v1/registries/name/aws.events/schemas/name/aws.a4b@RoomStateChange/language/Java8 (DescribeCodeBinding) -> 200 reporting the stored binding, so the write persisted and is observable to a later caller.
5. Compare steps 2 and 3: the same caller, naming the same AWS-owned schema, is refused by one write and served by the other, although state.rs:193-198 says every write that names a registry takes the same two steps and state.rs:3789-3790 says a caller "may read it but never change it".
