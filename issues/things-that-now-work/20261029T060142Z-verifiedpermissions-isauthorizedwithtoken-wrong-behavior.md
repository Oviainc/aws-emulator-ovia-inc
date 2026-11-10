Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4373217f8f4cfab6c9129dcbd57530e9c7b55ce8

# An OIDC accessTokenOnly source's `audiences` are matched against the token's `client_id` claim, not the `aud` claim the model names

_Filed: 2026-10-29T06:01:42Z_
_Operation: IsAuthorizedWithToken | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4373217f8f4cfab6c9129dcbd57530e9c7b55ce8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::token::check_audience` is the one place that decides whether a presented token was issued to a client the identity source accepts. For an OpenID Connect identity source configured with `accessTokenOnly`, it correctly selects the configured `audiences` list — and then compares that list against the WRONG claim: the claim is picked by token kind, and for `TokenKind::Access` that is `CLIENT_ID_CLAIM` (`"client_id"`), never `AUDIENCE_CLAIM` (`"aud"`).

The model says the configured values ARE the access token's `aud` claim values. And `matching_source` guarantees that an OIDC source whose `tokenSelection` is `accessTokenOnly` is only ever matched for a token presented in the `accessToken` member (`TokenKind::Access`), so for such a source the `aud` branch of that `match` is unreachable: the `audiences` list is NEVER compared against `aud`.

Consequences for `IsAuthorizedWithToken` against an OIDC `accessTokenOnly` source that configured a non-empty `audiences` list:

* a well-formed, correctly signed access token whose `aud` claim is exactly one of the configured audiences is REFUSED with the modelled `ValidationException` (`Failed to verify AccessToken: its client_id claim "" is not one of the audiences the identity source accepts`), because `read.claim_str("client_id")` returns nothing and `unwrap_or_default()` makes the compared value the empty string;
* conversely, a token whose `aud` is NOT one of the accepted audiences is ACCEPTED as long as it happens to carry a `client_id` claim whose value is in the list — so the audience restriction the identity source configured is not enforced at all.

The crate itself documents `client_id` as the Amazon-Cognito-specific claim, so the Cognito arm of the same function is right and only the OIDC access-token arm applies the Cognito rule to an OIDC source. The sibling OIDC arm is also right: `identityTokenOnly`'s `clientIds` are modelled as "The ID token audience, or client ID, claim values", which is the `aud` claim the `TokenKind::Identity` branch compares.

## Evidence

The model sentence that states the behaviour, from `smithy_query(query="shape", shape="OpenIdConnectAccessTokenConfiguration")` — member `audiences`:
```
<p>The access token <code>aud</code> claim values that you want to accept in your policy store. For example, <code>https://myapp.example.com, https://myapp2.example.com</code>.</p>
```
and the same shape's own documentation:
```
<p>The configuration of an OpenID Connect (OIDC) identity source for handling access token claims. Contains the claim that you want to identify as the principal in an authorization request, and the values of the <code>aud</code> claim, or audiences, that you want to accept.</p>
```

The code that does the opposite — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/state/token.rs:427-455 (the OIDC access-token arm selects `audiences`, then the claim is chosen by kind):
```
        (
            IdentitySourceConfiguration::OpenIdConnect(OpenIdConnectSource {
                token_selection: OpenIdConnectTokens::AccessToken { audiences, .. },
                ..
            }),
            _,
        ) => (audiences, "audiences"),
...
    let claim = match kind {
        TokenKind::Access => CLIENT_ID_CLAIM,
        TokenKind::Identity => AUDIENCE_CLAIM,
    };
    let claimed = read.claim_str(claim).unwrap_or_default();
    if allowed.iter().any(|allowed| allowed == claimed) {
        return Ok(());
    }
    Err(format!(
        "its {claim} claim {claimed:?} is not one of the {member} the identity source accepts"
    ))
```

What `CLIENT_ID_CLAIM` is, by the crate's OWN doc — src/state/token.rs:55-57:
```
/// The claim an Amazon Cognito ACCESS token names its app client in, which is that
/// token's audience by another name.
const CLIENT_ID_CLAIM: &str = "client_id";
```
(and src/state/token.rs:51-53 for the claim that is skipped: `/// The claim naming who a token was issued FOR (RFC 7519 §4.1.3) ... const AUDIENCE_CLAIM: &str = "aud";`)

Why the `aud` branch is unreachable for such a source — src/state/token.rs:383-390 (`matching_source`):
```
            IdentitySourceConfiguration::OpenIdConnect(oidc) => {
                oidc.issuer == issuer
                    && match (&oidc.token_selection, kind) {
                        (OpenIdConnectTokens::AccessToken { .. }, TokenKind::Access)
                        | (OpenIdConnectTokens::IdentityToken { .. }, TokenKind::Identity) => true,
                        (OpenIdConnectTokens::AccessToken { .. }, TokenKind::Identity)
                        | (OpenIdConnectTokens::IdentityToken { .. }, TokenKind::Access) => false,
                    }
            }
```

The handler that reaches it — src/ops/is_authorized_with_token.rs:262 (`crate::state::token::verify(&*self.engine, store, token, now)`), whose refusal becomes the modelled `ValidationException` at src/ops/is_authorized_with_token.rs:430-439.

Test coverage: nothing pins the OIDC audience comparison. `tests/all/is_authorized_with_token.rs` exercises only Cognito identity sources — its only audience rows are Cognito ones (`tests/all/is_authorized_with_token.rs:466` `"its aud claim {unlisted_client:?} is not one of the clientIds the identity ..."` and :474 `"its client_id claim {unlisted_client:?} is not one of the clientIds the ..."`), and `grep -n audiences tests/` finds `audiences` only in control-plane round-trip suites (`tests/all/identity_source_configuration_writers.rs:212-234`, `tests/all/identity_sources.rs:192`, `tests/all/list_identity_sources.rs:144`, `tests/all/update_and_delete_lifecycle.rs:130-147`), never in a token-verification test.

## Steps to Reproduce

1. CreatePolicyStore { validationSettings: { mode: "OFF" } } -> note policyStoreId PS1.
2. CreateIdentitySource { policyStoreId: PS1, principalEntityType: "MyCorp::User", configuration: { openIdConnectConfiguration: { issuer: "<an issuer this emulator publishes a key set for>", tokenSelection: { accessTokenOnly: { principalIdClaim: "sub", audiences: ["https://api.example.com"] } } } } } -> 200.
3. Present a correctly signed access token from that issuer whose claims are { iss: <issuer>, sub: "u1", aud: "https://api.example.com", exp: <in the future> } — i.e. its audience IS the one configured, and it carries no `client_id` claim (the usual shape of an OIDC access token).
4. IsAuthorizedWithToken { policyStoreId: PS1, accessToken: <that token>, action: {...}, resource: {...} }.
5. Observed: HTTP 400 ValidationException, message `Failed to verify AccessToken: its client_id claim "" is not one of the audiences the identity source accepts`, fieldList naming `/accessToken` — the token's `aud` claim was never looked at.
6. Expected per `OpenIdConnectAccessTokenConfiguration$audiences` ("The access token `aud` claim values that you want to accept in your policy store"): the token is accepted and the call returns an authorization decision.
7. Mirror case proving the restriction is also not enforced: present a token with claims { iss: <issuer>, sub: "u1", aud: "https://evil.example.com", client_id: "https://api.example.com", exp: <future> }. Observed: accepted and evaluated, although its `aud` is not an accepted audience.
