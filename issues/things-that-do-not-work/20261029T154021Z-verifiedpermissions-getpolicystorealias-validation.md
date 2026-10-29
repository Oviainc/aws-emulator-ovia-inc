Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 95be350e2e64bcff4ad89a5a8e5d1ae15d5b4a08

# GetPolicyStoreAlias answers 404 for an aliasName without the documented policy-store-alias/ prefix, the member rule its CreatePolicyStoreAlias sibling refuses with 400

_Filed: 2026-10-29T15:40:21Z_
_Operation: GetPolicyStoreAlias | Class: validation_

> _Note: this issue was identified against the code base at commit `95be350e2e64bcff4ad89a5a8e5d1ae15d5b4a08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetPolicyStoreAliasInput$aliasName` carries the same modelled note as the create's member: the name must always be prefixed with `policy-store-alias/`. `CreatePolicyStoreAlias` implements that rule explicitly — it refuses a name without the prefix with the modelled `ValidationException` naming `/aliasName`, and the crate's own comment explains why the modelled `pattern` cannot express the rule. This handler skips the check: it looks the raw value up in `state.aliases`, which is keyed by the FULL prefixed name, so an unprefixed value can never match a key and the caller is told the alias does not exist instead of that the name it sent is not a well-formed alias name.

The handler's own module doc states the input contract it does not enforce ("the alias name, prefixed with `policy-store-alias/`"), and its error enum declares no `ValidationException` at all, so — exactly as in the already-filed `DeletePolicyStoreAlias` case — there is no channel to refuse a malformed member with. A caller that drops the prefix gets a `ResourceNotFoundException` whose `resourceId` is the bare name, which reads as "this alias was deleted" rather than "fix the name".

## Evidence

Model (`smithy_query(query="shape", shape="GetPolicyStoreAliasInput")`, member `aliasName`): "Specifies the name of the policy store alias that you want information about." ... "The alias name must always be prefixed with <code>policy-store-alias/</code>."

The sibling in this crate that enforces exactly that rule — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/create_policy_store_alias.rs:161-176:
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

This handler, with no prefix check anywhere in the file — src/ops/get_policy_store_alias.rs:97-104:
```
        let alias = state.aliases.get(&input.alias_name).ok_or_else(|| {
            // `resourceId` is the alias's own name, without the resource path
            // its `aliasName` carries: an id a caller can feed back into the
            // API, which a prefixed one is not.
            GetPolicyStoreAliasErrorModeled::ResourceNotFoundException(
                crate::state::alias_reference(&input.alias_name).not_found(),
            )
        })?;
```
Its own doc states the contract it does not check — src/ops/get_policy_store_alias.rs:14-16:
```
//! * `aliasName` (Alias, len 0..150, pattern ^[a-zA-Z0-9-/_]*$) **required** — the
//!   alias name, prefixed with `policy-store-alias/`
//!   ([`crate::state::ALIAS_PREFIX`]).
```
and it has no channel to refuse with — src/ops/get_policy_store_alias.rs:35-43:
```
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#GetPolicyStoreAlias",
    extra_errors = ["InternalServerException"]
)]
pub enum GetPolicyStoreAliasErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
}
```
Why an unprefixed name can never match a key — src/state.rs:291-295: "Every policy store alias, keyed by its FULL `aliasName` — the name including its [`ALIAS_PREFIX`], exactly as the caller gave it".

Test coverage: tests/all/policy_store_alias.rs and tests/all/list_policy_store_aliases.rs cover the alias lifecycle; no case calls `GetPolicyStoreAlias` with an `aliasName` that omits the prefix, so nothing pins the current 404.

## Steps to Reproduce

1. CreatePolicyStore; note policyStoreId.
2. CreatePolicyStoreAlias with aliasName "policy-store-alias/example" and that policyStoreId — succeeds.
3. Call GetPolicyStoreAlias with aliasName "example" (the documented prefix omitted).
4. Observed: 404 ResourceNotFoundException with resourceId "example", i.e. the answer for an alias that does not exist.
5. Compare CreatePolicyStoreAlias with aliasName "example": 400 ValidationException whose fieldList names /aliasName and says it must be prefixed with policy-store-alias/ — the same member rule, enforced in one handler and not the other.
