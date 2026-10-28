Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 909749a7cea0ac224aeb09b9b62bd72ae5efa8b6

# CreatePolicy stores an empty-string name as a name, while its sibling UpdatePolicy normalises the same value to "no name"

_Filed: 2026-10-28T18:43:29Z_
_Operation: CreatePolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `909749a7cea0ac224aeb09b9b62bd72ae5efa8b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PolicyName` is modelled `len 0..150`, so `name: ""` reaches the handler. `CreatePolicy` persists the request member verbatim (`name: input.name.clone()`), so the policy is stored with `Some("")` — while `UpdatePolicy`, for the SAME member of the same resource, normalises `""` to no name at all. Three observable consequences inside this one crate:

1. `GetPolicy`/`ListPolicies` report `"name": ""` for the created policy, but report no `name` for a policy that reached the identical state through `UpdatePolicy` with `name: ""`.
2. The stored `Some("")` makes the per-store name-uniqueness check fire: a SECOND `CreatePolicy` carrying `name: ""` in the same store finds `p.name.as_deref() == Some("")` and is refused with `ConflictException`, although neither policy carries a name a caller can reference.
3. `PolicyStore::resolve_policy_id("name/")` resolves to that policy, i.e. the empty name becomes a usable reference.

The normalisation the crate already implements in `UpdatePolicy` is simply skipped on the create path.

## Evidence

Create path stores the empty string as a name — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/create_policy.rs:445-449:
```
        let policy = crate::state::Policy {
            policy_id: policy_id.clone(),
            policy_store_id: store_id.clone(),
            policy_type: policy_type.to_string(),
            name: input.name.clone(),
```
Sibling update path normalises the SAME member — src/ops/update_policy.rs:389-393:
```
        // "If you don't include the name in an update request, the existing name
        // is unchanged. To remove a name, set it to an empty string ("")."
        if let Some(name) = input.name.clone() {
            policy.name = (!name.is_empty()).then_some(name);
        }
```
The stored `Some("")` then triggers the conflict check — src/ops/create_policy.rs:422-437:
```
        // A policy name is unique within its store.
        if let Some(name) = input.name.as_deref() {
            if let Some(clash) = store
                .policies
                .values()
                .find(|p| p.name.as_deref() == Some(name))
```
Model text behind the update rule (`smithy_query(query="doc", shape="UpdatePolicyInput", member="name")`): "If you don't include the name in an update request, the existing name is unchanged. To remove a name, set it to an empty string (\"\")."

Test coverage: the update side of the rule is pinned by tests/all/policy_update.rs:270-275 (`("the empty string, which removes the name", Some(""), None)`); no test pins create-with-empty-name, so nothing in the crate justifies the divergence.

## Steps to Reproduce

1. CreatePolicyStore with validationSettings.mode = OFF.
2. CreatePolicy with a valid static statement and "name": "" — 200.
3. GetPolicy for the returned policyId: the response carries "name": "" (an update that submitted the same value would leave name absent).
4. CreatePolicy again in the same store with another statement and "name": "" — 409 ConflictException, although no policy in the store carries a usable name.
