Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f10a9c1d4031e9ac500bbdeee80ff5cb5da13a14

# PutSchema with `{}` stores an empty schema instead of deleting the schema, so a STRICT store that deleted its schema stops refusing Cedar writes

_Filed: 2026-10-29T00:05:40Z_
_Operation: PutSchema | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f10a9c1d4031e9ac500bbdeee80ff5cb5da13a14`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`SchemaDefinition$cedarJson` documents `{}` as the way to DELETE a store's schema: "To delete the schema, run PutSchema with `{}` for this parameter." This crate states the consequence of that itself, as the doc on its own `EMPTY_SCHEMA_JSON` constant: "It is also the document `PutSchema` returns a store to … so \"no schema\" and \"the empty schema\" are one state of one document."

The `PutSchema` handler does not implement that. It writes `store.schema = Some(StoredSchema { cedar_json: "{}" , … })` for every definition, including `{}`, so a store whose schema was "deleted" holds `Some(..)` while a store that never had one holds `None`. The crate then keys the STRICT-mode rule off exactly that distinction: `PolicyStore::schema_to_validate_against` answers `Err` ("its validation mode is STRICT and it contains no schema…") only for `None`, and for `Some("{}")` answers `Ok(Some("{}"))` — which sends the submitted policy through Cedar's validator against an empty schema instead.

So two stores that this service reports IDENTICALLY through `GetSchema` (schema `"{}"`, no namespaces — `schema_document()` manufactures exactly that document for the `None` case) answer every STRICT Cedar write differently:

* STRICT + never had a schema -> HTTP 400 `ValidationException`, `fieldList[0].path = "/policyStoreId"`, the refusal the model's `ValidationSettings` sentence requires ("If Mode=STRICT and the policy store doesn't contain a schema, Verified Permissions rejects all static policies and policy templates because there is no schema to validate against");
* STRICT + schema deleted with `{}` -> the refusal is skipped entirely and `validate_static_policy` / `validate_policy_template` runs against the empty document, so the answer depends on what Cedar makes of a policy under a schema that declares nothing rather than on the documented rule. A policy that names no entity type or action (`permit(principal, action, resource);`) has nothing in it for an empty schema to reject, so it is accepted and STORED in a store the model says must reject every static policy.

The divergence reaches `CreatePolicy` (create_policy.rs:324), `UpdatePolicy` (update_policy.rs:299), `CreatePolicyTemplate` (create_policy_template.rs:265) and `UpdatePolicyTemplate` (update_policy_template.rs:262) — every write that asks the store what to validate against.

## Evidence

The model sentence that makes `{}` a DELETE — `smithy_query(query="doc", shape="SchemaDefinition", member="cedarJson")`:
```
<p>A JSON string representation of the schema supported by applications that use this policy store. To delete the schema, run <a …>PutSchema</a> with <code>{}</code> for this parameter. …</p>
```
and the rule a schema-less STRICT store must follow — `smithy_query(query="shape", shape="ValidationSettings")`, member `mode`:
```
<important> <p>If <code>Mode=STRICT</code> and the policy store doesn't contain a schema, Verified Permissions rejects all static policies and policy templates because there is no schema to validate against. </p> …
```

The crate's OWN statement of the same equivalence — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/state.rs:441-446:
```
/// The EMPTY Cedar schema document: a schema that declares no namespace, and the
/// document a policy store holds until `PutSchema` writes one. It is also the
/// document `PutSchema` returns a store to — `SchemaDefinition$cedarJson`
/// documents "To delete the schema, run PutSchema with `{}` for this parameter" —
/// so "no schema" and "the empty schema" are one state of one document.
pub const EMPTY_SCHEMA_JSON: &str = "{}";
```
and again on the reader — src/state.rs:940-953:
```
    /// A store that has never been given a schema holds the EMPTY document
    /// ([`EMPTY_SCHEMA_JSON`]), dated at the store's own creation: the schema is
    /// one document of the store, in the state a caller also returns it to by
    /// putting `{}`. …
    pub fn schema_document(&self) -> StoredSchema {
        self.schema.clone().unwrap_or_else(|| StoredSchema {
            cedar_json: EMPTY_SCHEMA_JSON.to_string(),
```

The handler that breaks it — src/ops/put_schema.rs:177-182 and 204-212:
```
        // The namespaces are a property of the schema text, so they come from the
        // real Cedar schema parser; text it refuses is refused here too, with its
        // own diagnostics. `{}` is a well-formed schema that declares nothing,
        // which is how the model documents clearing a store's definitions ("To
        // delete the schema, run PutSchema with `{}` for this parameter"): it is
        // stored, and reports no namespaces.
```
```
        store.schema = Some(crate::state::StoredSchema {
            // The document, not the caller's spelling of it: …
            cedar_json: crate::state::canonical_schema_json(definition),
            namespaces: parsed.namespaces,
            created_date,
            last_updated_date: now,
        });
```
(the comment says the put is "clearing a store's definitions", and the line under it stores `Some(..)` anyway.)

The branch that then treats the two states differently — src/state.rs:976-993:
```
    pub fn schema_to_validate_against(
        &self,
        content: ValidatedContent,
    ) -> Result<Option<&str>, crate::ops::InvalidInput> {
        if self.validation_mode != VALIDATION_MODE_STRICT {
            return Ok(None);
        }
        match self.schema.as_ref() {
            None => Err(crate::ops::InvalidInput::new(
                crate::ops::paths::POLICY_STORE_ID,
                format!(
                    "its validation mode is STRICT and it contains no schema, so there is \
                     nothing to validate {} against",
                    content.noun()
                ),
            )),
            Some(schema) => Ok(Some(schema.cedar_json.as_str())),
        }
    }
```

Test coverage: the `{}` put IS exercised, but only against an `OFF` store — tests/all/schema.rs:188 (`let store_id = store_with_mode(&router, "OFF").await;`) with the row at tests/all/schema.rs:200:
```
        ("emptied", json!({}), &[]),
```
and the suite that pins the STRICT-with-no-schema rule enumerates only two ways a store reaches that state, not this third one — tests/all/store_validation_settings.rs:174-177:
```
/// The rows span the four write operations the rule covers, both kinds of content
/// (a policy and a policy template), and both ways a store reaches the state (one
/// created `STRICT`, and one switched to `STRICT` after it was given content —
/// the only way such a store can hold a policy to update at all).
```
So nothing pins or justifies the schema-deleted-with-`{}` behaviour.

## Steps to Reproduce

1. CreatePolicyStore { validationSettings: { mode: "STRICT" } } -> note policyStoreId PS_A (never given a schema).
2. CreatePolicyStore { validationSettings: { mode: "STRICT" } } -> note policyStoreId PS_B.
3. PutSchema { policyStoreId: PS_B, definition: { cedarJson: "{\"PhotoApp\":{\"entityTypes\":{},\"actions\":{}}}" } } -> 200.
4. PutSchema { policyStoreId: PS_B, definition: { cedarJson: "{}" } } -> 200, namespaces = [] (the documented way to delete the schema).
5. GetSchema on PS_A and on PS_B: both report schema = "{}" and namespaces = [] — the service says the two stores hold the same document.
6. CreatePolicy { policyStoreId: PS_A, definition: { static: { statement: "permit(principal, action, resource);" } } } -> HTTP 400 ValidationException, fieldList[0].path = "/policyStoreId", message "its validation mode is STRICT and it contains no schema, so there is nothing to validate a policy against".
7. CreatePolicy { policyStoreId: PS_B, definition: { static: { statement: "permit(principal, action, resource);" } } } -> the "no schema" refusal is NOT raised: the handler takes the `Ok(Some("{}"))` branch and validates against the empty document, and the policy is created and readable through GetPolicy/ListPolicies.
8. Expected: identical answers for PS_A and PS_B — per `SchemaDefinition$cedarJson` the `{}` put DELETED PS_B's schema, and per `ValidationSettings$mode` a STRICT store that "doesn't contain a schema" rejects all static policies and policy templates. The crate's own src/state.rs:441-446 says these are "one state of one document".
9. The same split applies to CreatePolicyTemplate, UpdatePolicy and UpdatePolicyTemplate against PS_A vs PS_B.
