Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 5b375a973f59b1ba0f091f974f00cc96321aba54

# CreatePolicy checks the store's quota before the duplicate-name conflict, against the ordering CreatePolicyTemplate states for the same pair of checks

_Filed: 2026-10-28T20:55:35Z_
_Operation: CreatePolicy | Class: error-response_

> _Note: this issue was identified against the code base at commit `5b375a973f59b1ba0f091f974f00cc96321aba54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreatePolicyTemplate` states explicitly that the duplicate-name `ConflictException` is decided BEFORE the store's quota, so a name that already exists is reported as the documented conflict whether or not the store is also at its bound. `CreatePolicy` runs the two checks in the opposite order: the `ServiceQuotaExceededException` branch comes first and returns, so the name-conflict branch below it is never reached for a store that is at `MAX_POLICIES_PER_STORE` or at `MAX_POLICY_BYTES_PER_STORE`.

For the same caller mistake (a `name` another policy in the store already holds) the two sibling create handlers therefore answer different modelled errors depending only on how full the store is: `CreatePolicyTemplate` answers 409 `ConflictException`, `CreatePolicy` answers 402 `ServiceQuotaExceededException` and never tells the caller the name is taken.

## Evidence

The ordering rule, stated in the sibling — `src/ops/create_policy_template.rs:273`:

```
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
```

followed by the quota at `src/ops/create_policy_template.rs:295`:

```
        if store.policy_templates.len() >= crate::state::MAX_POLICY_TEMPLATES_PER_STORE
            || store.cedar_bytes() + input.statement.len()
                > crate::state::MAX_POLICY_BYTES_PER_STORE
```

The handler that breaks that order — quota first, `src/ops/create_policy.rs:385`:

```
        if store.policies.len() >= crate::state::MAX_POLICIES_PER_STORE
            || store.cedar_bytes() + statement.as_ref().map_or(0, String::len)
                > crate::state::MAX_POLICY_BYTES_PER_STORE
        {
            return Err(CreatePolicyErrorModeled::ServiceQuotaExceededException(
```

and only then the name conflict it can no longer reach, `src/ops/create_policy.rs:405`:

```
        // A policy name is unique within its store.
        if let Some(name) = input.name.as_deref() {
            if let Some(clash) = store
                .policies
                .values()
                .find(|p| p.name.as_deref() == Some(name))
            {
                return Err(CreatePolicyErrorModeled::ConflictException(
```

## Steps to Reproduce

1. CreatePolicyStore with validationSettings.mode OFF.
2. CreatePolicy in it with {"name": "taken", "definition": {"static": {"statement": "permit(principal, action, resource);"}}}.
3. Fill the same store with static policies until its Cedar text reaches crate::state::MAX_POLICY_BYTES_PER_STORE (src/state.rs:69).
4. CreatePolicy again with the SAME {"name": "taken"}: the response is 402 ServiceQuotaExceededException, never the 409 ConflictException that names the colliding policy.
5. Repeat with CreatePolicyTemplate on a store at MAX_POLICY_TEMPLATES_PER_STORE and a duplicate template name: that handler answers 409 ConflictException, the order its own comment at src/ops/create_policy_template.rs:273 states.
