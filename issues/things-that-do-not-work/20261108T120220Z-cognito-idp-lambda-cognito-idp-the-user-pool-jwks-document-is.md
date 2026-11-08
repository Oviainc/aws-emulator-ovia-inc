Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__lambda.pre-token-generation-v2-trigger-adds-suppresses-and-scopes-access-token-claims
Issue-Key: cognito-idp__lambda.pre-token-generation-v2-trigger-adds-suppresses-and-scopes-access-token-claims|_verify_rs256#21|KeyError|n = _b64url_to_int(jwk["n"])
Issue-Commit: 63b1f768cca526e685e8991a6e4ba2a43400c274

# cognito-idp: the user-pool JWKS document is served but its JWK entries omit the RSA modulus `n` (and exponent `e`), so no issued token can be verified

> _Note: this issue was identified against the code base at commit `63b1f768cca526e685e8991a6e4ba2a43400c274`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
jwks.json (user pool signing keys endpoint)

## Description
One net-new root cause, grounded in a failing check the replay report marked neither ALREADY FILED nor OWNED.

Cause 1 of 1 — GET <cognito-idp endpoint>/<userPoolId>/.well-known/jwks.json now answers, but the JWK it returns has no `n` member, so RS256 verification of a pool-issued token is impossible.
AWS: recorded behavior only (the JWKS route is an OIDC endpoint, not a modeled Smithy operation) — every user pool publishes https://cognito-idp.<region>.amazonaws.com/<userPoolId>/.well-known/jwks.json as {"keys": [{"alg": "RS256", "e": "<base64url exponent>", "kid": "<key id>", "kty": "RSA", "n": "<base64url modulus>", "use": "sig"}, ...]}. The `kid` in the header of an issued IdToken/AccessToken is present in that set, and its `n`/`e` pair reconstructs the public key that verifies the token's RS256 signature. This is how every resource server, API Gateway JWT authorizer and OIDC client validates a Cognito token.
Emulator: the fetched JWK is missing the modulus — the check's verifier raises KeyError: 'n' at n = _b64url_to_int(jwk["n"]) — so the document is structurally present but carries no usable key material and no token the emulator mints can be verified.
Why: token verification is the foundation every relying party stands on. A JWKS entry without `n`/`e` is not a key: every client that fetches it fails while parsing rather than at verification, and claim-shaping behavior (here, a PreTokenGeneration V2_0 trigger adding, suppressing and scoping access-token claims) cannot be observed at all because the token can never be accepted.
Fix: the cognito-idp JWKS serializer — emit kty/alg/kid/use plus the base64url-encoded `n` and `e` of the pool's RSA signing key, with the same kid and algorithm the token minter stamps into issued tokens.
Related already-open issues, neither of which covers this: 20261024T114943Z-cognito-idp-lambda-the-pool-s-jwks-document.md records as its Cause 1 that this route answered HTTP 404 and carries this check's replay key for that HTTPError — the route now answers, so that key no longer matches and the remaining defect, an incomplete JWK, is outside it; 20261029T182116Z-cognito-idp-issued-tokens-diverge-es256-instead-of.md records the token algorithm and the absent JWKS document for the cognito-idp service token. A fixer could close either with the modulus still missing.
Failing check: cognito-idp__lambda.pre-token-generation-v2-trigger-adds-suppresses-and-scopes-access-token-claims.

## Evidence
KeyError: 'n', raised at check line 178 in _verify_rs256 (n = _b64url_to_int(jwk["n"])), reached from _verify_jwt_against_jwks at line 199 (assert _verify_rs256(signing_input, signature, jwk) is True). Expected the JWKS entry selected by the token header's kid to carry base64url `n` and `e` so the RS256 signature verifies; observed a JWK with no `n`. The route no longer 404s — the previously filed HTTPError no longer occurs — so the surviving divergence is the key document's contents.

## Steps to Reproduce
CreateUserPool with a PreTokenGeneration V2_0 Lambda trigger and CreateUserPoolClient; AdminCreateUser/AdminSetUserPassword and sign in to obtain an AccessToken; decode the token header to read its `kid`; HTTP GET <cognito-idp endpoint>/<userPoolId>/.well-known/jwks.json -> expect 200 and a `keys` entry for that kid containing kty='RSA', alg='RS256', and base64url `n` and `e` that verify the token's signature; observe a JWK entry with no `n` member (KeyError on lookup).
