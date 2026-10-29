Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6dde6b5b59a4735e67d87b555115fcc549deec6e

# DeletePolicyStore resolves a `policy-store-alias/<name>` reference and destroys the aliased store, although its own modelled member says the alias name cannot be used

_Filed: 2026-10-28T22:23:44Z_
_Operation: DeletePolicyStore | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6dde6b5b59a4735e67d87b555115fcc549deec6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeletePolicyStoreInput$policyStoreId` is the ONE `policyStoreId` member in this API that the model explicitly restricts to an ID: "To specify a policy store, the alias name cannot be used. Only the ID can be used." (The only other member with that restriction is `CreatePolicyStoreAliasInput$policyStoreId`, and this crate honours it there — explicitly, with a comment naming the sentence and a test row pinning the refusal.)

`DeletePolicyStore`'s handler nevertheless resolves the reference through `state.resolve_store_id`, the helper whose own doc says it resolves "either a store id, or `policy-store-alias/<name>` naming one". So `DeletePolicyStore { policyStoreId: "policy-store-alias/my-alias" }` resolves the alias to the store behind it and then unconditionally destroys it, together with every policy, policy template, identity source, schema and tag it holds, plus every alias that pointed at it.

Per the model note, `"policy-store-alias/my-alias"` is not an alias reference for this operation at all — it is just an id, and no policy store has it. The operation is `@idempotent` and documents "If you specify a policy store that does not exist, the request response will still return a successful HTTP 200 status code", which is the branch the handler already has three lines above (`audit(... "already absent")` -> `Ok`). So the correct answer for an alias-shaped reference is that no-op 200 — not the deletion of a store the caller addressed by a form the model excludes.

This is also the one place where the restriction matters most: every other alias-accepting member is a read or a non-destructive write, while here the divergence silently deletes customer state. And because `resolve_store_id` only resolves an `Active` alias, the destruction is reachable precisely for the aliases a caller is most likely to have in hand.

## Evidence

The model sentence, from `smithy_query(query="doc", shape="DeletePolicyStoreInput", member="policyStoreId")`:
```
<p>Specifies the ID of the policy store that you want to delete.</p> <note> <p>To specify a policy store, the alias name cannot be used. Only the ID can be used.</p> </note>
```
The handler's own module doc repeats it verbatim — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/delete_policy_store.rs:30-32:
```
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store that you want to delete. To specify a policy store,
//!   the alias name cannot be used. Only the ID can be used.
```
…and the code under it resolves aliases anyway — src/ops/delete_policy_store.rs:129-139:
```
        // No such store is a success: `DeletePolicyStore` is `@idempotent` and
        // models no not-found error, so a repeat delete must not fail.
        let Some(store_id) = state.resolve_store_id(&input.policy_store_id) else {
            crate::ops::audit(
                ctx,
                "DeletePolicyStore",
                &input.policy_store_id,
                "already absent",
            );
            return Ok(DeletePolicyStoreOutput {});
        };
```
followed by the destruction — src/ops/delete_policy_store.rs:161-167:
```
        // The store owns its policies, so removing it removes them too, and the
        // aliases that named it stop resolving.
        state.policy_stores.remove(&store_id);
        state
            .aliases
            .retain(|_, alias| alias.policy_store_id != store_id);
        crate::ops::audit(ctx, "DeletePolicyStore", &store_id, "deleted");
```
What `resolve_store_id` does, stated by the helper it calls — src/state.rs:834-847:
```
    /// Resolve a `policyStoreId` request member: either a store id, or
    /// `policy-store-alias/<name>` naming one.
    …
    pub fn resolve_store_id(&self, reference: &str) -> Option<String> {
        self.lookup_store_id(reference).ok()
    }
```
The sibling in THIS crate that implements the same model sentence correctly — src/ops/create_policy_store_alias.rs:181-191:
```
        // `policyStoreId` is documented as an ID here — "The alias name cannot be
        // used" — so it is looked up directly instead of through
        // `resolve_store_id`, which would also accept an alias reference.
        if !state.policy_stores.contains_key(&input.policy_store_id) {
            return Err(
                CreatePolicyStoreAliasErrorModeled::ResourceNotFoundException(
                    crate::ops::resource_not_found(&input.policy_store_id, "POLICY_STORE"),
                )
                .into(),
            );
        }
```
Test coverage: the sibling's rule IS pinned — tests/all/policy_store_alias.rs:588-592:
```
    let bad_stores: &[(&str, String)] = &[
        ("unknown store id", "PSnope".to_string()),
        ("a store that was deleted", deleted_store),
        ("an alias reference, which this member refuses", live_alias),
    ];
```
(expected 404 / `ResourceNotFoundException`). `DeletePolicyStore`'s own suite pins only id-shaped references: tests/all/policy_store_authorization.rs:196-234 passes `id` and `"PSneverexisted"`, and `grep -n alias tests/all/policy_store_authorization.rs` returns nothing — so no test pins or justifies the alias-resolving behaviour.

## Steps to Reproduce

1. CreatePolicyStore { validationSettings: { mode: "OFF" } } -> note policyStoreId PS1.
2. CreatePolicy { policyStoreId: PS1, definition: { static: { statement: "permit(principal, action, resource);" } } } -> note policyId SP1.
3. CreatePolicyStoreAlias { aliasName: "policy-store-alias/my-alias", policyStoreId: PS1 } -> 200, alias is Active.
4. DeletePolicyStore { policyStoreId: "policy-store-alias/my-alias" }.
5. Observed: HTTP 200 — and the store is GONE: GetPolicyStore { policyStoreId: PS1 } now answers 404 ResourceNotFoundException, GetPolicy { policyStoreId: PS1, policyId: SP1 } answers 404, and GetPolicyStoreAlias { aliasName: "policy-store-alias/my-alias" } answers 404 because the alias record was swept with the store.
6. Expected per `DeletePolicyStoreInput$policyStoreId` ("the alias name cannot be used. Only the ID can be used") plus the operation's documented idempotent no-op for a store that does not exist: HTTP 200 with NOTHING deleted — PS1, SP1 and the alias all still readable.
7. Contrast the sibling that reads the same sentence: CreatePolicyStoreAlias { aliasName: "policy-store-alias/other", policyStoreId: "policy-store-alias/my-alias" } correctly answers 404 instead of following the alias.
