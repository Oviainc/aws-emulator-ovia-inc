Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8135e4312fd85a54e50f448892daaf011c09f669

# CreateIdentitySource renders its idempotency fingerprint without the escaping its sibling creates use, so two different requests sharing a clientToken collide and the second is answered as a replay

_Filed: 2026-10-28T20:03:59Z_
_Operation: CreateIdentitySource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8135e4312fd85a54e50f448892daaf011c09f669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every `@idempotencyToken` create in this crate compares a canonical rendering of the first call's parameters against the current call's: equal means "replay, answer with the first result", different means `ConflictException`. `CreatePolicy` and `CreatePolicyTemplate` build that rendering with the crate's own `crate::state::idempotency_parameters`, whose doc states the rule it exists for: every value goes through `{:?}` so that a member whose value contains the `=` or `;` separators cannot make two different requests render alike. `CreatePolicyStore` hand-rolls its rendering but still applies that rule (`{:?}` on every caller-supplied value).

`CreateIdentitySource` is the one create that hand-rolls the rendering AND drops the rule: it interpolates `principalEntityType` with a plain `{}`, and the configuration fingerprint it appends interpolates the OIDC `issuer` (and `principalIdClaim`, `userPoolArn`) with a plain `{}` too, while using the very `=`/`;` characters as its own separators. Both of those members are caller-supplied strings whose modelled patterns admit `;` and `=` (`PrincipalEntityType` = `^.*$`, `Issuer` = `^https://.*$`).

Consequence: two genuinely different `CreateIdentitySource` requests carrying the same `clientToken` can render to the identical `parameters` string. The second one is then taken for a replay — it returns the FIRST call's `identitySourceId`/`createdDate` with HTTP 200, the identity source it asked for is never created and never persisted, and the `ConflictException` the model documents for a token reused with different parameters is never raised. A documented error is turned into a success, and a create is silently lost.

## Evidence

The rule, stated by the helper the other creates use — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/state.rs:1227-1240:
```
/// A canonical, UNAMBIGUOUS rendering of the parameters one `clientToken` was
/// first used with, for the [`IdempotencyRecord::parameters`] field: a later
/// call carrying the same token but a different rendering is the
/// `ConflictException` the model documents.
///
/// Every value is written through `{:?}`, which quotes and escapes it, so a
/// member whose value contains the `=` or `;` separators cannot make two
/// different requests render alike.
pub fn idempotency_parameters(members: &[(&str, Option<&str>)]) -> String {
    members
        .iter()
        .map(|(name, value)| format!("{name}={value:?};"))
        .collect()
}
```
Siblings that call it: src/ops/create_policy.rs:263 (`PolicyDefinition::Static(def) => crate::state::idempotency_parameters(&[`) and src/ops/create_policy_template.rs:217 (`let parameters = crate::state::idempotency_parameters(&[`). `CreatePolicyStore` hand-rolls but keeps the escaping — src/ops/create_policy_store.rs:86-89:
```
    format!(
        "mode={};description={:?};deletionProtection={:?};tags={:?}",
        input.validation_settings.mode, input.description, input.deletion_protection, tags
    )
```
(`mode` is the only plainly interpolated value and it is an enum, `OFF | STRICT`.)

The path that breaks the rule — src/ops/create_identity_source.rs:281-284:
```
        let parameters = format!(
            "store={store_id};principalEntityType={principal_entity_type};configuration={}",
            configuration.fingerprint()
        );
```
and the fingerprint it appends — src/state.rs:493-496:
```
                format!(
                    "oidc:issuer={};entityIdPrefix={:?};groups={:?};tokenSelection={}",
                    oidc.issuer, oidc.entity_id_prefix, groups, tokens
                )
```
`principal_entity_type` and `oidc.issuer` are plainly interpolated, and the model lets both carry the separators: smithy_query(query="shape", shape="PrincipalEntityType") -> `"pattern": "^.*$"`, length 1..200; shape="Issuer" -> `"pattern": "^https://.*$"`, length 1..2048.

A concrete collision, both rows legal input (same policyStoreId, same clientToken, OIDC `identityTokenOnly` with `principalIdClaim: "c"` and no `clientIds`/`entityIdPrefix`/`groupConfiguration`):
- Request A: `principalEntityType = "X"`, `issuer = "https://a;configuration=oidc:issuer=https://b"`
- Request B: `principalEntityType = "X;configuration=oidc:issuer=https://a"`, `issuer = "https://b"`

Both render to the same string:
`store=PS…;principalEntityType=X;configuration=oidc:issuer=https://a;configuration=oidc:issuer=https://b;entityIdPrefix=None;groups=None;tokenSelection=identityTokenOnly:claim=c;clientIds=[]`

so the replay branch at src/ops/create_identity_source.rs:285-312 takes `record.parameters == parameters` and returns A's source:
```
        if let Some(token) = input.client_token.as_deref() {
            if let Some(record) = state.idempotency_replay("CreateIdentitySource", token) {
                if record.parameters != parameters {
                    … ConflictException …
                }
                if let Some(source) = state
                    .policy_stores
                    .get(&store_id)
                    .and_then(|store| store.identity_sources.get(&record.resource_id))
                {
                    return Ok(output_for(source));
                }
```
The model documents the error this skips: smithy_query(query="operation", operation="CreateIdentitySource") lists `ConflictException (HTTP 409)` for the operation, and `crate::state::IdempotencyRecord::parameters` (src/state.rs:312-314) says "A canonical rendering of that first call's parameters. A later call with the same token but a different rendering is a `ConflictException`."

Test coverage: tests/all/identity_sources.rs exercises CreateIdentitySource creation and the OIDC configuration rows (e.g. the `oidc-identity-token-with-prefix-and-groups` case at line 215) but no row uses a `principalEntityType` or `issuer` containing `;`/`=`, so nothing pins the ambiguous rendering.

## Steps to Reproduce

1. CreatePolicyStore { validationSettings: { mode: "OFF" } } -> note policyStoreId PS.
2. CreateIdentitySource { policyStoreId: PS, clientToken: "tok1", principalEntityType: "X", configuration: { openIdConnectConfiguration: { issuer: "https://a;configuration=oidc:issuer=https://b", tokenSelection: { identityTokenOnly: { principalIdClaim: "c" } } } } } -> 200, note identitySourceId IS-1.
3. CreateIdentitySource { policyStoreId: PS, clientToken: "tok1", principalEntityType: "X;configuration=oidc:issuer=https://a", configuration: { openIdConnectConfiguration: { issuer: "https://b", tokenSelection: { identityTokenOnly: { principalIdClaim: "c" } } } } }.
4. Observed: HTTP 200 echoing IS-1 (the first call's identitySourceId and createdDate). Expected per the crate's own rule for a token reused with different parameters: ConflictException (HTTP 409).
5. GetIdentitySource / ListIdentitySources for PS: only IS-1 exists — step 3's identity source was never persisted, and its `principalEntityType` and `issuer` were silently discarded.
6. Contrast with the sibling: the same trick against CreatePolicy (a `name`/`statement` containing `;` or `=` with a reused clientToken) correctly conflicts, because create_policy.rs:263 renders through `crate::state::idempotency_parameters`.
