Issue-Severity: HIGH
Issue-Gap-Class: error-response
Issue-Commit: 66b81b2d38066f623e620cf9384f94dad963db50

# Every OpenID Connect identity source is unusable: IsAuthorizedWithToken answers 500 InternalServerException for a token its issuer signed, because the key-set lookup only knows Cognito user pools

_Filed: 2026-10-29T12:06:00Z_
_Operation: IsAuthorizedWithToken | Class: error-response_

> _Note: this issue was identified against the code base at commit `66b81b2d38066f623e620cf9384f94dad963db50`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateIdentitySource`/`UpdateIdentitySource` deliberately accept and persist an `openIdConnectConfiguration` whose `issuer` is any https URL (the crate's own translation helper says the issuer "names a provider outside the emulator and is accepted as given"), and `GetIdentitySource`/`ListIdentitySources` report it back. But the only operation an identity source exists to serve cannot use it: `crate::state::token::verify` requires `published_key_set(engine, issuer)` to succeed, and that function resolves an issuer ONLY by looking it up as an Amazon Cognito user pool in the cognito-idp service's state (`pools.pool(&user_pool_id)` plus `pool.issuer() == issuer`). For any issuer that is not a Cognito pool issuer URL — i.e. every issuer an `openIdConnectConfiguration` would realistically name — it returns `None`, which becomes `TokenRefusal::NotCheckable`, which `IsAuthorizedWithToken` maps to `InternalServerException` (HTTP 500).

So a fully well-formed request — a store that exists, an OIDC identity source whose issuer matches the token's `iss`, the right token member for the source's `tokenSelection`, a live token — is answered with a server fault instead of an ALLOW/DENY decision. Nothing downstream of the key-set lookup is reachable for an OIDC source: `check_audience`'s `audiences`/`clientIds` arms, `check_window`, the `principalIdClaim` principal, the `entityIdPrefix` prefixing and the OIDC `groupConfiguration` group mapping are all dead code for a real OIDC provider. The crate admits the gap in a comment rather than attempting anything for it, which is why this is reported as a defect rather than a feature request.

## Evidence

The crate accepts an external OIDC issuer on purpose — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/mod.rs:400-402:
```
/// A Cognito pool reference goes through [`crate::state::resolve_user_pool_arn`],
/// the one implementation of THAT rule; an OIDC `issuer` names a provider outside
/// the emulator and is accepted as given.
```

The comment that admits no such issuer can ever be served — src/state/token.rs:660-661:
```
/// `None` when nothing in this process publishes a key set for `issuer`, which is every
/// issuer outside the emulator.
```
and the body that makes it so — src/state/token.rs:666-675:
```
pub fn published_key_set<Eng: Send + Sync + 'static>(engine: &Eng, issuer: &str) -> Option<Value> {
    let pools = ::service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice
        ::state::service_state(engine)?;
    let user_pool_id = issuer.rsplit('/').next()?.to_string();
    let pools = pools.lock();
    let pool = pools.pool(&user_pool_id)?;
    (pool.issuer() == issuer).then(|| pool.jwks())
}
```

Where that `None` becomes the answer — src/state/token.rs:307-312:
```
    let key_set = published_key_set(engine, &issuer).ok_or_else(|| {
        TokenRefusal::NotCheckable {
            issuer: issuer.clone(),
            reason: "it publishes no key set this service can read".to_string(),
        }
    })?;
```
and src/ops/is_authorized_with_token.rs:453-461:
```
        crate::state::token::TokenRefusal::NotCheckable { issuer, reason } => {
            IsAuthorizedWithTokenErrorModeled::InternalServerException(InternalServerException {
                message: format!(
                    "the signature of the token the issuer {issuer} minted could not be \
                     checked: {reason}"
                ),
            })
```
The handler's own module doc states the same outcome at src/ops/is_authorized_with_token.rs:21-23: "A signature this service cannot CHECK at all — an issuer whose key set it cannot read, a key it cannot check a signature under — is the modelled `InternalServerException`, because nothing was decided about the token."

That the identity source exists to serve this operation is the model's own sentence — smithy_query(query="doc", shape="com.amazonaws.verifiedpermissions#DeleteIdentitySource"): "After you delete the identity source, you can no longer use tokens for identities from that identity source to represent principals in authorization queries made using IsAuthorizedWithToken."

Note the issuer matching that precedes the key-set lookup succeeds for an OIDC source — src/state/token.rs:383-384: `IdentitySourceConfiguration::OpenIdConnect(oidc) => { oidc.issuer == issuer && ... }` — so the store really does hold a source for the token's issuer, and the 500 is not a missing-resource answer.

Test coverage: tests/all/is_authorized_with_token.rs builds every token against a Cognito pool issuer (`fn pool_issuer(pool: &str) -> String { format!(...cognito-idp...) }`, line 264) and has no row for an `openIdConnectConfiguration` source at all; the OIDC source rows in tests/all/identity_sources.rs, tests/all/identity_source_configuration_writers.rs and tests/all/update_and_delete_lifecycle.rs only create/read/update such a source and never authorize with it. So nothing pins or justifies the 500.

## Steps to Reproduce

1. Boot the emulator and point a Verified Permissions client at its data listener.
2. store = CreatePolicyStore(validationSettings={"mode":"OFF"}).
3. CreateIdentitySource(policyStoreId=store, principalEntityType="MyCorp::User", configuration={"openIdConnectConfiguration":{"issuer":"https://auth.example.com","tokenSelection":{"accessTokenOnly":{"principalIdClaim":"sub"}}}}) -> 200, an identitySourceId is returned.
4. GetIdentitySource(policyStoreId=store, identitySourceId=<id>) -> 200, the OIDC configuration is reported back, so the source is real persisted state.
5. CreatePolicy(policyStoreId=store, definition={"static":{"statement":"permit(principal, action, resource);"}}).
6. IsAuthorizedWithToken(policyStoreId=store, accessToken=<any JWT whose iss is "https://auth.example.com">, action={...}, resource={...}).
7. Observed: HTTP 500 InternalServerException, message "the signature of the token the issuer https://auth.example.com minted could not be checked: it publishes no key set this service can read" — no decision, no principal, no errors list.
8. Expected: the operation computes a decision (or, for a bad token, the modelled 400 ValidationException / 404 ResourceNotFoundException the handler already builds). Contrast: the identical call against a Cognito-backed identity source answers 200 with ALLOW, as tests/all/is_authorized_with_token.rs shows.
