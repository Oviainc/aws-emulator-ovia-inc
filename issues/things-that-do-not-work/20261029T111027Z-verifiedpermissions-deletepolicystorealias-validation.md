Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 9ca5ab623cd27c1b2d04fc113bc9b809b7e6f27d

# DeletePolicyStoreAlias reports a successful delete for an aliasName without the documented policy-store-alias/ prefix, which its CreatePolicyStoreAlias sibling refuses

_Filed: 2026-10-29T11:10:27Z_
_Operation: DeletePolicyStoreAlias | Class: validation_

> _Note: this issue was identified against the code base at commit `9ca5ab623cd27c1b2d04fc113bc9b809b7e6f27d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeletePolicyStoreAliasInput$aliasName` carries the same modelled note as the create's member — the name must always carry the `policy-store-alias/` prefix. `CreatePolicyStoreAlias` implements that rule explicitly (it refuses a name without the prefix with the modelled `ValidationException`, and the crate's own comment explains why the modelled `pattern` cannot express it). `DeletePolicyStoreAlias` skips the same check entirely.

Because `state.aliases` is keyed by the FULL alias name (prefix included), an unprefixed `aliasName` can never match a key, so the handler takes its "already absent" branch and answers an empty HTTP 200 — reporting the delete as done. The alias the caller meant to delete is left `Active` and keeps resolving in every `policyStoreId` member, and the caller has no signal that anything was wrong: a mistyped or prefix-less name is indistinguishable, on the wire, from a delete that actually happened.

The handler's own error enum reinforces that the check was never considered: it declares only `InvalidStateException` and `InternalServerException`, so unlike its create sibling it has no channel to refuse a malformed member at all.

## Evidence

Model, `smithy_query(query="shape", shape="DeletePolicyStoreAliasInput")` — member `aliasName`:
  "Specifies the name of the policy store alias that you want to delete." ... "The alias name must always be prefixed with `policy-store-alias/`."

Sibling that enforces it — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/create_policy_store_alias.rs:161-176:
```
        // The documented prefix is a rule the model's `pattern` cannot express —
        // it admits any `[a-zA-Z0-9-/_]*` — and a name without it could never be
        // referenced through a `policyStoreId` member, so the request is refused
        // rather than persisted as unreachable.
        if !input.alias_name.starts_with(crate::state::ALIAS_PREFIX) {
            return Err(CreatePolicyStoreAliasErrorModeled::ValidationException(
                crate::ops::validation_exception([crate::ops::InvalidInput::new(
                    crate::ops::paths::ALIAS_NAME,
```

This handler — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/delete_policy_store_alias.rs:121-129 (no prefix check anywhere in the file):
```
        if !state.aliases.contains_key(&input.alias_name) {
            crate::ops::audit(
                ctx,
                "DeletePolicyStoreAlias",
                &input.alias_name,
                "already absent",
            );
            return Ok(DeletePolicyStoreAliasOutput {});
        }
```

Why an unprefixed name can never match — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/state.rs:291-295:
```
    /// Every policy store alias, keyed by its FULL `aliasName` — the name
    /// including its [`ALIAS_PREFIX`], exactly as the caller gave it, which is
    /// also the form a `policyStoreId` member carries when it names an alias.
```
and src/state.rs:43-48, which quotes the same model note for the constant: "The prefix every policy store alias name carries, from `CreatePolicyStoreAliasInput$aliasName`'s own model documentation: \"The alias name must always be prefixed with `policy-store-alias/`.\""

No channel to refuse with — src/ops/delete_policy_store_alias.rs:51-59:
```
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#DeletePolicyStoreAlias",
    extra_errors = ["InternalServerException"]
)]
pub enum DeletePolicyStoreAliasErrorModeled {
    InvalidStateException(InvalidStateException),
    InternalServerException(InternalServerException),
}
```

Test coverage: tests/all/policy_store_alias.rs and tests/all/list_policy_store_aliases.rs exercise the alias lifecycle; no case sends a `DeletePolicyStoreAlias` whose `aliasName` omits the prefix, so nothing pins the current empty-200 answer.

## Steps to Reproduce

1. CreatePolicyStore, note its policyStoreId.
2. CreatePolicyStoreAlias with aliasName "policy-store-alias/example" and that policyStoreId — succeeds.
3. Call DeletePolicyStoreAlias with aliasName "example" (the same name without the documented `policy-store-alias/` prefix).
4. Observe HTTP 200 with an empty body — the same answer a real delete gives.
5. Call GetPolicyStoreAlias with aliasName "policy-store-alias/example": the alias is still present and still `Active`, and it still resolves in a `policyStoreId` member (e.g. GetPolicyStore), so the reported success deleted nothing.
6. Compare with CreatePolicyStoreAlias using aliasName "example": that call answers 400 ValidationException with fieldList naming /aliasName.
