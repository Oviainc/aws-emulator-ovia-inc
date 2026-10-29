Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: cbb9267c7abd2d15d6f906cf892ef291750ce9f0

# DeletePolicy answers 404 for a policy that does not exist, where the model documents a successful HTTP 200

_Filed: 2026-10-28T19:13:26Z_
_Operation: DeletePolicy | Class: error-response_

> _Note: this issue was identified against the code base at commit `cbb9267c7abd2d15d6f906cf892ef291750ce9f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents DeletePolicy as idempotent in the strongest terms: deleting a policy that does not exist must answer HTTP 200. The handler does the opposite — it resolves the `policyId` against the store's `policies` map and, when nothing resolves, returns `ResourceNotFoundException` (404). So a retry of a delete that already succeeded, or any idempotent teardown that deletes without first checking, fails instead of succeeding.

The same sentence appears verbatim in this handler's own module documentation (copied from the model by the scaffold), so the file states the rule its code breaks. The sibling `DeletePolicyStore` in this same crate implements exactly this rule for the same reason — its handler returns `Ok` for an unresolvable store reference with the comment "a repeat delete must not fail" — so the crate is internally inconsistent about which delete honours the documented idempotency.

Note the modelled `ResourceNotFoundException` stays reachable after a fix: the `policyStoreId` member can still name a store that does not exist (that case has no documented 200), which is what the other not-found rows of the error table exercise.

## Evidence

Smithy (smithy_query doc, shape="DeletePolicy"): "Deletes the specified policy from the policy store." ... "This operation is idempotent; if you specify a policy that doesn't exist, the request response returns a successful <code>HTTP 200</code> status code."

The same sentence in the handler's own header — services/.../src/ops/delete_policy.rs:22-24: "//! Deletes the specified policy from the policy store. This operation is idempotent; if you //! specify a policy that doesn't exist, the request response returns a successful HTTP 200 //! status code."

The code that contradicts it — services/.../src/ops/delete_policy.rs:126-148:
```
        let policy_not_found = || {
            DeletePolicyErrorModeled::ResourceNotFoundException(
                crate::state::policy_reference(&input.policy_id).not_found(),
            )
        };
...
        let policy_id = store
            .resolve_policy_id(&input.policy_id)
            .ok_or_else(policy_not_found)?;
        store.policies.remove(&policy_id);
```
(`resolve_policy_id` answers `Some` only for a policy that is PRESENT — src/state.rs:974-986.)

Sibling that implements the identical documented rule — services/.../src/ops/delete_policy_store.rs:129-139:
```
        // No such store is a success: `DeletePolicyStore` is `@idempotent` and
        // models no not-found error, so a repeat delete must not fail.
        let Some(store_id) = state.resolve_store_id(&input.policy_store_id) else {
            ... return Ok(DeletePolicyStoreOutput {});
        };
```

Test coverage that pins the current (contradicting) answer, with no reason stated for diverging from the documented 200 — tests/all/error_messages.rs:313-320: `case: "DeletePolicy, live store and unknown policy id"` ... asserts 404 / `ResourceNotFoundException`; and tests/all/wire_fidelity.rs:408-414 `"DeletePolicy, absent policy name reference"`. Both rows would need to move to the store-not-found form.

## Steps to Reproduce

1. CreatePolicyStore -> note policyStoreId.
2. CreatePolicy in that store -> note policyId.
3. DeletePolicy { policyStoreId, policyId } -> 200 (the policy is removed).
4. DeletePolicy again with the SAME policyId (or any unknown policyId, or `name/absent`) against the live store.
5. Observed: HTTP 404 ResourceNotFoundException with resourceType POLICY. Documented: a successful HTTP 200 status code.
