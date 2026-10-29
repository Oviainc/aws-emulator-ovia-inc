Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 5b375a973f59b1ba0f091f974f00cc96321aba54

# The identity-source configuration translation is implemented twice, once per identity-source writer

_Filed: 2026-10-28T20:55:35Z_
_Operation: CreateIdentitySource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `5b375a973f59b1ba0f091f974f00cc96321aba54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`persisted_configuration` — the function that reduces a request's identity-provider union to the stored `crate::state::IdentitySourceConfiguration`, including the Cognito pool-ARN resolution and the OIDC token-selection mapping — exists twice in this crate: once in `create_identity_source.rs` over `Configuration`, once in `update_identity_source.rs` over `UpdateConfiguration`. The two bodies are line-for-line the same logic over two member-identical generated unions, so any rule added to one copy (a new member, a new refusal, a changed default for an absent list) silently does not apply to the other writer of the same stored field.

The crate already treats this exact situation the other way round on the read side: `list_identity_sources.rs` re-types what `get_identity_source.rs` produced precisely so "there is ONE place that decides what a read says about an identity source". The write side has no such single place.

Existing coverage that must stay unchanged: `tests/all/identity_sources.rs` (the create-side pool-resolution rows, including `the_signed_region_decides_which_user_pools_resolve` at line 422 and the unresolvable-pool rows at line 334) and `tests/all/update_and_delete_lifecycle.rs::an_updated_identity_source_is_read_back_as_the_new_provider` (line 110), whose rows cover both union arms and both token selections through the update copy.

## Evidence

Copy 1 — `src/ops/create_identity_source.rs:135`:

```
pub(crate) fn persisted_configuration(
    configuration: &Configuration,
    request_region: &str,
) -> Result<crate::state::IdentitySourceConfiguration, crate::ops::InvalidInput> {
    match configuration {
        Configuration::cognitoUserPoolConfiguration(cognito) => {
            let (region, user_pool_id) =
                crate::state::resolve_user_pool_arn(&cognito.user_pool_arn, request_region)?;
```

Copy 2 — `src/ops/update_identity_source.rs:145`, whose own doc admits the two are the same translation:

```
/// `UpdateConfiguration` is member-identical to the `Configuration` a create
/// carries, so this translates the same provider description into the same
/// stored shape — and a Cognito pool reference goes through
/// [`crate::state::resolve_user_pool_arn`], the one implementation of that rule,
/// so an update accepts exactly the pools a create accepts.
fn persisted_configuration(
    configuration: &UpdateConfiguration,
    request_region: &str,
) -> Result<crate::state::IdentitySourceConfiguration, crate::ops::InvalidInput> {
    match configuration {
        UpdateConfiguration::cognitoUserPoolConfiguration(cognito) => {
            let (region, user_pool_id) =
                crate::state::resolve_user_pool_arn(&cognito.user_pool_arn, request_region)?;
```

Both then build the identical `CognitoUserPoolSource { user_pool_arn, user_pool_id, region, client_ids: ….unwrap_or_default(), group_entity_type }` (create:144-154, update:153-164) and the identical `OpenIdConnectSource { issuer, entity_id_prefix, group_configuration, token_selection }` with the same `accessTokenOnly`/`identityTokenOnly` mapping (create:156-184, update:166-194).

The single-place discipline the read side already follows, `src/ops/list_identity_sources.rs:250`:

```
/// `configuration` and the deprecated `details` are whatever
/// [`crate::ops::get_identity_source`] reports for the same stored source,
/// re-typed into this operation's own shapes: … so there is ONE place that
/// decides what a read says about an identity source.
```

## Steps to Reproduce

1. Open src/ops/create_identity_source.rs:135-186 and src/ops/update_identity_source.rs:145-196 side by side: the two match arms, the resolve_user_pool_arn call, the unwrap_or_default on client_ids/audiences and the token-selection mapping are the same logic twice.
2. Note that the only difference is the union type matched on (Configuration vs UpdateConfiguration), which update_identity_source.rs:140 itself describes as "member-identical".
3. Add a rule to one copy (e.g. a refusal for an unresolvable OIDC issuer) and observe it does not apply to the other writer of the same stored crate::state::IdentitySourceConfiguration.
4. Run tests/all/identity_sources.rs and tests/all/update_and_delete_lifecycle.rs: they exercise both copies, so a single shared translation must leave both suites passing byte-for-byte.
