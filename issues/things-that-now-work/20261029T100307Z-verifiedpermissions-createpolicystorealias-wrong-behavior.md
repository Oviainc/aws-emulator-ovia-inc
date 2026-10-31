Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1d0d0c28dcee365463f48da791546814f4205587

# Alias names are unique across the whole account instead of per account AND Region, so a name free in a second Region is refused 409 and resolves to the first Region's store

_Filed: 2026-10-29T10:03:07Z_
_Operation: CreatePolicyStoreAlias | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1d0d0c28dcee365463f48da791546814f4205587`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreatePolicyStoreAliasInput$aliasName` documents the uniqueness SCOPE of an alias name as the account *and* Region. The handler enforces a stricter, Region-less rule: the alias table is keyed by the alias name alone, with no Region component, so (a) a `CreatePolicyStoreAlias` signed for Region B naming a name already held in Region A is refused with `ConflictException` although that name is unused in Region B, and (b) the single stored alias is resolved and reported from every Region, so a `policyStoreId` of `policy-store-alias/<name>` sent to Region B resolves to the store created in Region A and `GetPolicyStoreAlias`/`ListPolicyStoreAliases` answer an `aliasArn` naming a Region the caller did not address.

The crate contradicts itself on exactly this point: the same handler reads the request's Region and bakes it into the alias ARN *because* the name is Region-scoped, while the state field it writes into is documented as keyed by the bare name.

## Evidence

Model (smithy_query doc, CreatePolicyStoreAliasInput$aliasName): "Specifies the name of the policy store alias to create. The name must be unique within your Amazon Web Services account and Amazon Web Services Region."

Handler doc, services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/create_policy_store_alias.rs:20-22:
```
//! * **The name is unique in the account and Region** — a request naming a name
//!   another store already holds conflicts (HTTP 409), which is what makes
//!   concurrent creations of one name have exactly one winner.
```
Same file, create_policy_store_alias.rs:233-240 — the Region is read off the request for the ARN, for that very reason:
```
            // The alias ARN names the region the request was signed for: an
            // alias name is unique in the account and Region, so the ARN that
            // identifies it has to say which Region.
            arn: crate::state::alias_arn(
                &crate::state::request_region(ctx.parts()),
                &input.alias_name,
            ),
```
But the lookup and the insert are keyed by the name only — create_policy_store_alias.rs:198-212:
```
        if let Some(existing) = state.aliases.get(&input.alias_name) {
            if existing.state == crate::state::ALIAS_STATE_ACTIVE
                && existing.policy_store_id == input.policy_store_id
            { ... }
            return Err(already_taken(existing).into());
        }
```
and create_policy_store_alias.rs:245: `state.aliases.insert(input.alias_name.clone(), alias);`

The state field itself, services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/state.rs:291-295:
```
    /// Every policy store alias, keyed by its FULL `aliasName` — the name
    /// including its [`ALIAS_PREFIX`], exactly as the caller gave it, which is
    /// also the form a `policyStoreId` member carries when it names an alias. A
    /// `BTreeMap` so `ListPolicyStoreAliases` can enumerate in a stable order.
    pub aliases: BTreeMap<String, PolicyStoreAlias>,
```
Resolution is likewise Region-less — state.rs:893-908 (`lookup_store_id`) matches `self.aliases.get(reference)` with no Region in the key.

Test coverage: tests/all/policy_store_alias.rs and tests/all/list_policy_store_aliases.rs exercise aliases from a single signing Region only, so no existing test pins the cross-Region outcome.

## Steps to Reproduce

1. Boot the emulator and point a Verified Permissions client signing for us-west-2 at it.
2. storeA = CreatePolicyStore(validationSettings={"mode":"OFF"}); CreatePolicyStoreAlias(aliasName="policy-store-alias/shared", policyStoreId=storeA) -> 200, aliasArn carries us-west-2.
3. Re-point the client to sign for eu-west-1; storeB = CreatePolicyStore(validationSettings={"mode":"OFF"}).
4. CreatePolicyStoreAlias(aliasName="policy-store-alias/shared", policyStoreId=storeB) -> 409 ConflictException, although the model says the name only has to be unique within the account AND Region and no alias of that name exists in eu-west-1.
5. Still signing for eu-west-1, call GetPolicyStoreAlias(aliasName="policy-store-alias/shared") -> 200 reporting storeA and an aliasArn naming us-west-2; and GetPolicyStore(policyStoreId="policy-store-alias/shared") -> storeA, a store created in another Region.
