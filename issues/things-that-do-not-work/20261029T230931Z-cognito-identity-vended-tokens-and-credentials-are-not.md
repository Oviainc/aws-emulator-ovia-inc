Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-identity-role-mapping-configuration
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-identity-role-mapping-configuration cognito-identity-principal-tag-attribute-mapping cogid-creds-custom-role-arn-outside-mappings-rejected cogid-creds-developer-authenticated-credentials-envelope cogid-creds-unauth-credentials-envelope cogid-merge-developer-login-of-other-identity-refused cogid-merge-getid-blank-absorbed-by-registered-developer-login cogid-merge-idp-proven-identity-login-conflicts cogid-merge-losing-identity-disabled-after-merge cogid-merge-same-public-provider-different-users-conflict cogid-merge-token-duration-above-max-rejected cogid-merge-token-duration-zero-rejected cognito-identity-developer-authenticated-identity-flow cognito-identity-merge-developer-identities-workflow cognito-identity-basic-identity-and-openid-token-flow cognito-identity-get-openid-token-error-lifecycle cognito-identity-get-openid-token-lifecycle cognito-identity-unauthenticated-guest-identity-flow
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|SetIdentityPoolRoles|4xx->2xx|$
Issue-Key: cognito-identity|SetIdentityPoolRoles|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|SetIdentityPoolRoles|4xx->2xx|status
Issue-Key: cognito-identity|SetPrincipalTagAttributeMap|2xx->4xx|$.IdentityPoolId
Issue-Key: cognito-identity|SetPrincipalTagAttributeMap|2xx->4xx|$.IdentityProviderName
Issue-Key: cognito-identity|SetPrincipalTagAttributeMap|2xx->4xx|$.PrincipalTags
Issue-Key: cognito-identity|SetPrincipalTagAttributeMap|2xx->4xx|$.UseDefaults
Issue-Key: cognito-identity|SetPrincipalTagAttributeMap|2xx->4xx|status
Issue-Key: cognito-identity|GetCredentialsForIdentity|2xx->2xx|$.Credentials.AccessKeyId
Issue-Key: cognito-identity|GetOpenIdTokenForDeveloperIdentity|2xx->2xx|$.Token
Issue-Key: cognito-identity|GetOpenIdToken|2xx->2xx|$.Token
Issue-Commit: 2019710a3c4752758402c7d5e42a18d73361dfac

# cognito-identity: vended tokens and credentials are not AWS-shaped and provider config checks invert

> _Note: this issue was identified against the code base at commit `2019710a3c4752758402c7d5e42a18d73361dfac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetIdentityPoolRoles

## Description
This issue bundles 5 root causes: two where the provider-configuration check on a pool-roles / principal-tag write is inverted relative to AWS, and three where the material the emulator vends (an access key id, an OpenID token) is not shaped like what AWS returns. Fix: the SetIdentityPoolRoles and SetPrincipalTagAttributeMap provider-validation paths, and the credential/token minting component behind GetCredentialsForIdentity, GetOpenIdToken and GetOpenIdTokenForDeveloperIdentity.

Cause 1 of 5: SetIdentityPoolRoles carrying a RoleMappings entry for a provider that is not configured on the pool. AWS: 400, `__type` InvalidParameterException, header `x-amzn-errortype: InvalidParameterException:`, message '(<provider-name>) is not a valid RoleMapping ProviderName or is not a configured provider.' — fixed text with the provider name from the request in parentheses. Emulator: HTTP 200 with an empty body; the unconfigured role mapping is stored. Why: a pool accepts a role mapping that AWS would reject, so a configuration error surfaces only later (or never) and credential vending follows a mapping AWS would not have. Fix: provider-configuration validation in the SetIdentityPoolRoles handler. Traces: cognito-identity-role-mapping-configuration#task#5.

Cause 2 of 5: SetPrincipalTagAttributeMap for an IdentityProviderName that is not configured on the pool. AWS: HTTP 200 echoing IdentityPoolId, IdentityProviderName, PrincipalTags ({'aws:subject': 'sub'}) and UseDefaults=false — AWS accepts the mapping. Emulator: HTTP 400, no echo. Why: this is the inverse of cause 1 — the emulator refuses a write AWS accepts, so client setup of attribute mappings fails outright. Fix: remove the provider-configuration precondition from the SetPrincipalTagAttributeMap handler (AWS does not apply it). Traces: cognito-identity-principal-tag-attribute-mapping#task#8.

Cause 3 of 5: GetCredentialsForIdentity. AWS: Credentials.AccessKeyId is an STS temporary-credential access key id — the 'ASIA' prefix followed by the key body. Emulator: it returns a key id built from its own literal prefix ('EMULATOR...'), which is not a valid STS access-key-id shape. Why: anything that validates or classifies the credential by its key-id prefix (SDK credential scope checks, SigV4 tooling, IAM-style assertions) rejects or mis-handles the session. Fix: the credential-minting component behind GetCredentialsForIdentity. Traces: cogid-creds-custom-role-arn-outside-mappings-rejected#task#7 (3 steps). Related open issues without these replay keys: '20261028T040721Z-cognito-identity-cognito-idp-iam-getcredentialsforidentity-vends-an.md', '20261029T071507Z-cognito-identity-getcredentialsforidentity-under-a-token-role-mapping.md'.

Cause 4 of 5: GetOpenIdTokenForDeveloperIdentity. AWS: Token is a signed JWS — three dot-separated segments whose header names a real signing algorithm and whose signature segment is non-empty. Emulator: it returns a JWT with header {"alg":"none","typ":"JWT"} and an EMPTY signature segment. Why: any relying party that verifies the token (STS AssumeRoleWithWebIdentity, a JWT library with signature verification on) rejects it, so the developer-authenticated flow cannot be completed against a verifier. Fix: the OpenID token minting component (sign the token; publish a resolvable key). Traces: cogid-creds-developer-authenticated-credentials-envelope#task#5 (20 steps). Related open issue without the replay key: '20261029T173827Z-cognito-identity-10-net-new-token-credential-refusal.md'.

Cause 5 of 5: GetOpenIdToken. AWS: Token is a signed JWS as in cause 4 (the recording binds it as the value later flows consume). Emulator: an unsigned alg=none JWT with an empty signature segment. Why/Fix: as cause 4, on the GetOpenIdToken path. Traces: cognito-identity-basic-identity-and-openid-token-flow#task#3 (4 steps).

Note: the particular key id, token payload, identity ids and iat/exp values in the evidence are per-run values, not requirements; the requirements are the access-key-id shape ('ASIA' temporary-credential form) and that the token is a signed JWS with a non-empty signature.

## Evidence
Replay of simparity-wire-aws-v1, expected vs actual:
- SetIdentityPoolRoles: expected 400 with {'__type': 'InvalidParameterException', 'message': '(graph.facebook.com) is not a valid RoleMapping ProviderName or is not a configured provider.'} and x-amzn-errortype 'InvalidParameterException:', got 200 with an empty body (cognito-identity-role-mapping-configuration#task#5).
- SetPrincipalTagAttributeMap: expected 200 with $.IdentityPoolId, $.IdentityProviderName ('cognito-idp.<region>.amazonaws.com/<user-pool-id>'), $.PrincipalTags {'aws:subject': 'sub'} and $.UseDefaults false, got 400 (cognito-identity-principal-tag-attribute-mapping#task#8).
- GetCredentialsForIdentity: $.Credentials.AccessKeyId did not satisfy the recorded access-key-id shape — got 'EMULATORAS2VGOW7SSDL' (cogid-creds-custom-role-arn-outside-mappings-rejected#task#7, 3 steps).
- GetOpenIdTokenForDeveloperIdentity: $.Token did not satisfy the recorded token shape — got a JWT whose header decodes to {"alg":"none","typ":"JWT"} with an empty third segment (cogid-creds-developer-authenticated-credentials-envelope#task#5, 20 steps).
- GetOpenIdToken: $.Token mismatch against the bound token value — got an alg=none JWT with an empty signature segment (cognito-identity-basic-identity-and-openid-token-flow#task#3, 4 steps).
Smithy: Credentials.AccessKeyId -> AccessKeyString; GetOpenIdTokenResponse/GetOpenIdTokenForDeveloperIdentityResponse.Token -> String (no shape constraint), so the token/key shape requirement is recorded behavior; SetIdentityPoolRoles errors include InvalidParameterException (service-common); SetPrincipalTagAttributeMap errors are NotAuthorizedException and ResourceNotFoundException only.

## Steps to Reproduce
1. Replay cognito-identity-role-mapping-configuration step task#5: SetIdentityPoolRoles with a RoleMappings key for a provider not configured on the pool -> emulator answers 200; AWS answers 400 InvalidParameterException.
2. Replay cognito-identity-principal-tag-attribute-mapping#task#8: SetPrincipalTagAttributeMap for an unconfigured IdentityProviderName -> emulator answers 400; AWS answers 200 with the echo.
3. Replay cogid-creds-custom-role-arn-outside-mappings-rejected#task#7 and inspect Credentials.AccessKeyId: it starts with the emulator's literal prefix instead of the STS 'ASIA' temporary-credential form.
4. Replay cogid-creds-developer-authenticated-credentials-envelope#task#5 and cognito-identity-basic-identity-and-openid-token-flow#task#3 and base64-decode the token header: alg is 'none' and the signature segment is empty.
