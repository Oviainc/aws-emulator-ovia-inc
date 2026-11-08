Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.forged-access-tokens-are-refused-and-verifiably-unsigned cognito-idp.limit-exceeded-resource-server
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.forged-access-tokens-are-refused-and-verifiably-unsigned|_verify_rs256#23|KeyError|n = int.from_bytes(_b64url_decode(jwk["n"]), "big")
Issue-Key: service-quotas|GetAWSDefaultServiceQuota|NoSuchResourceException
Issue-Commit: 5bdc422e571ff089197010dd8b23a6f4d1e72235

# cognito-idp: 2 net-new causes — the pool's published JWK carries no RSA modulus so no issued token can be verified, and Service Quotas knows no cognito-idp quota catalog

> _Note: this issue was identified against the code base at commit `5bdc422e571ff089197010dd8b23a6f4d1e72235`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetUser

## Description
Two net-new root causes, both grounded in failing cognito-idp checks.

Cause 1 of 2: the pool's JWKS document is served and contains an entry whose "kid" matches the issued access token's header, but the entry carries no RSA modulus member "n" (and, by the same omission, no "e"), so a client cannot verify any signature the pool produces. Failing check: cognito-idp.forged-access-tokens-are-refused-and-verifiably-unsigned.

  AWS:      recorded behavior only (the JWKS endpoint is not in the Smithy model): a user pool publishes https://<cognito-idp host>/<userPoolId>/.well-known/jwks.json as an RFC 7517 JWK Set in which each key is an RSA public key with kty="RSA", use="sig", alg="RS256", a "kid" matching the `kid` header of the tokens it signs, and the base64url members "n" (modulus) and "e" (exponent) — the two values an RS256 verifier needs. Smithy: GetUser / UpdateUserAttributes take a required AccessToken (TokenModelType, pattern ^[A-Za-z0-9-_=.]+$) and model NotAuthorizedException (HTTP 403), which is the refusal class a forged or unverifiable token must land in.
  Emulator: the served JWK for the token's kid is present but has no "n" member, so RS256 verification cannot even be attempted — the check's verifier raises `KeyError: 'n'` while establishing its positive control (the GENUINE token's signature must verify against the published key before the three forgeries are judged). Consequently the whole forged-token refusal taxonomy behind it (alg:none, an injected cognito:groups claim under the original signature, and a one-byte signature flip, each of which AWS refuses on GetUser and UpdateUserAttributes with 403 NotAuthorizedException and the fixed messages "Invalid Access Token" for the unsigned variant and "Could not verify signature for Access Token" for the two RS256 forgeries — no variable parts) is unverifiable here.
  Why:      every API gateway, library and service that validates Cognito tokens offline fetches this document and builds an RSA key from n/e. With n absent, every such client fails closed (or, worse, is tempted to skip verification), so a pool's tokens cannot be trusted by anything but Cognito itself, and an application cannot distinguish a genuine token from a forgery.
  Fix:      the user-pool signing-key publication path — the JWKS document serializer and the key material it reads must emit the RSA public key's "n" and "e" (base64url, unpadded) alongside kty/use/alg/kid for the key whose kid the issued tokens carry. Related open issue 20261108T120220Z-cognito-idp-lambda-cognito-idp-the-user-pool-jwks-document-is.md reports the same omission under the cognito-idp__lambda heading and does not carry this check's replay key, so it does not cover it; 20261029T182116Z-cognito-idp-issued-tokens-diverge-es256-instead-of.md and 20261024T114943Z-cognito-idp-lambda-the-pool-s-jwks-document.md are the earlier token/JWKS issues. All should be fixed together.
  Traces:   cognito-idp.forged-access-tokens-are-refused-and-verifiably-unsigned.

Cause 2 of 2: Service Quotas has no entry for the Cognito user-pool quotas, so even the AWS-default lookup for "resource servers per user pool" is refused, and a caller cannot discover the limit the service claims to enforce. Failing check: cognito-idp.limit-exceeded-resource-server.

  AWS:      Smithy (services/service-com-amazonaws-servicequotas-servicequotasv20190624): GetServiceQuota and GetAWSDefaultServiceQuota both take required ServiceCode + QuotaCode and answer 200 with Quota (ServiceQuota: ServiceCode, QuotaCode, QuotaName, Value, Adjustable, GlobalQuota, Unit); NoSuchResourceException (HTTP 404, documented "The specified resource does not exist") is modeled for a service/quota pair that does not exist. ServiceCode="cognito-idp" with QuotaCode="L-7CDAF993" is an existing, documented and adjustable quota (resource servers per user pool), so GetAWSDefaultServiceQuota always answers 200 with its AWS default Value even on an account that never raised it; GetServiceQuota answers 200 for the applied value (NoSuchResourceException there is only legitimate when the quota has no account-level override). The quota is what cognito-idp enforces when CreateResourceServer is called once too often — HTTP 400 LimitExceededException.
  Emulator: GetServiceQuota(ServiceCode="cognito-idp", QuotaCode="L-7CDAF993") is refused NoSuchResourceException and so is the fallback GetAWSDefaultServiceQuota, with Error.Code=NoSuchResourceException and Error.Message="The request failed because the specified quota and service do not exist." — i.e. the pair is reported as non-existent rather than as merely un-overridden. The caller never reaches the CreateResourceServer limit behaviour it was going to grade.
  Why:      a client that reads its own limits before provisioning (the documented way to avoid pinning a default AWS does not guarantee) is told the quota does not exist, so it cannot size its work, cannot tell "no override" from "unknown quota", and any limit the emulator does enforce on resource servers is undiscoverable.
  Fix:      the Service Quotas quota catalog/state consulted by the GetServiceQuota and GetAWSDefaultServiceQuota handlers — it must contain the cognito-idp service's quota entries (at least L-7CDAF993) with the AWS default Value, Adjustable=true and the modeled ServiceQuota members, and reserve NoSuchResourceException for genuinely unknown service/quota pairs.
  Traces:   cognito-idp.limit-exceeded-resource-server.

## Evidence
Cause 1 — check cognito-idp.forged-access-tokens-are-refused-and-verifiably-unsigned, verdict [error].
Actual (replay output):
  File "check_cognito_idp_forged_access_tokens_are_refused_and_verifiably_unsigned.py", line 84, in run
    assert _verify_rs256(f"{header_b64}.{payload_b64}".encode(),
  File "check_cognito_idp_forged_access_tokens_are_refused_and_verifiably_unsigned.py", line 203, in _verify_rs256
    n = int.from_bytes(_b64url_decode(jwk["n"]), "big")
  KeyError: 'n'
Expected: GET <endpoint>/<userPoolId>/.well-known/jwks.json returns {"keys":[{"kty":"RSA","use":"sig","alg":"RS256","kid":"<the kid in the token header>","n":"<base64url modulus>","e":"AQAB"}, ...]} and the genuine access token's RS256 signature verifies against it; the three forgeries then fail verification and are refused by GetUser and UpdateUserAttributes with HTTP 403 NotAuthorizedException ("Invalid Access Token" for alg:none, "Could not verify signature for Access Token" for the two RS256 forgeries).
Actual: the kid-matching JWK is served without "n", so no verifier can be built; the key values and kid are per-run, the presence of n/e is the requirement.

Cause 2 — check cognito-idp.limit-exceeded-resource-server, verdict [error].
Actual (replay output):
  botocore.errorfactory.NoSuchResourceException: An error occurred (NoSuchResourceException) when calling the GetAWSDefaultServiceQuota operation: The request failed because the specified quota and service do not exist.
Expected: GetServiceQuota(ServiceCode="cognito-idp", QuotaCode="L-7CDAF993") answers 200 with Quota.Value (or NoSuchResourceException only for a missing account override, in which case GetAWSDefaultServiceQuota answers 200 with the AWS default Value, Adjustable=true, QuotaName naming resource servers per user pool).
Actual: HTTP 404 NoSuchResourceException from BOTH operations, so the quota is undiscoverable and the CreateResourceServer LimitExceededException behaviour behind it is never exercised.

## Steps to Reproduce
Cause 1:
1. CreateUserPool + CreateUserPoolClient (ExplicitAuthFlows ALLOW_USER_PASSWORD_AUTH, no secret); AdminCreateUser + AdminSetUserPassword(Permanent).
2. InitiateAuth USER_PASSWORD_AUTH and split the returned AccessToken; read `kid` from its base64url header.
3. GET <cognito-idp endpoint>/<userPoolId>/.well-known/jwks.json and select the key with that kid.
   Expected: an RSA JWK with "n" and "e", against which the token's signature verifies.
   Observed: the entry has no "n" member, so verification raises KeyError: 'n'.

Cause 2:
1. Call service-quotas GetServiceQuota(ServiceCode="cognito-idp", QuotaCode="L-7CDAF993").
2. On NoSuchResourceException, call GetAWSDefaultServiceQuota with the same pair.
   Expected: 200 with the quota's default Value (the adjustable "resource servers per user pool" quota).
   Observed: HTTP 404 NoSuchResourceException — "The request failed because the specified quota and service do not exist."
