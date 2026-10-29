Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-identity-basic-identity-and-openid-token-flow
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-identity-basic-identity-and-openid-token-flow cognito-identity-create-identity-pool-invalid-name-rejected cognito-identity-identity-pool-lifecycle-crud cogid-login-list-identities-next-token-pagination cognito-identity-list-identities-max-results-out-of-range-rejected cognito-identity-validation-error-boundaries cogid-login-list-identity-pools-next-token-pagination cogid-merge-getid-blank-absorbed-by-registered-developer-login cognito-identity-get-credentials-for-identity-errors cognito-identity-unauthenticated-guest-identity-flow cogid-merge-token-duration-above-max-rejected cogid-merge-token-duration-zero-rejected cognito-identity-get-openid-token-error-lifecycle
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|GetId|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|CreateIdentityPool|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|ListIdentityPools|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|GetOpenIdTokenForDeveloperIdentity|4xx->4xx|$headers.x-amzn-errortype
Issue-Related: 20261029T230931Z-cognito-identity-identity-scoped-refusals-answer-403-404.md
Issue-Related: 20261029T230931Z-cognito-identity-pool-scoped-not-found-refusals-answer.md
Issue-Commit: 2019710a3c4752758402c7d5e42a18d73361dfac

# cognito-identity: error responses omit the x-amzn-errortype header

> _Note: this issue was identified against the code base at commit `2019710a3c4752758402c7d5e42a18d73361dfac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetId

## Description
This issue bundles 7 root causes that are the same serializer defect observed on 7 different operations: the emulator's error responses carry the right status and the right awsJson1_1 body, but no `x-amzn-errortype` response header at all. AWS: for awsJson1_1 services every modeled error response carries `x-amzn-errortype: <ErrorCode>:` (the shape name followed by a colon, e.g. `ValidationException:`, `ResourceNotFoundException:`, `InvalidParameterException:`) alongside the `__type` member in the body; recorded behavior shows that header present on every error step below. Smithy: protocols = awsJson1_1, common errors InternalErrorException/InvalidParameterException/TooManyRequestsException plus the per-operation modeled errors. Emulator: header absent on every error response. Why: SDKs and proxies that resolve the error class from the header (and anything that logs or routes on it) fall back to the status code or fail to classify the error at all, so a client sees a generic failure instead of the modeled exception. Fix: the shared awsJson1_1 error serializer for this service — emit `x-amzn-errortype` for every error it writes; no per-operation change is needed. Each cause below is one cluster where the header is the ONLY divergence, so each is independently verifiable.

Cause 1 of 7: GetId refused for an invalid request. AWS: header `x-amzn-errortype: ValidationException:`. Emulator: header missing. Traces: cognito-identity-basic-identity-and-openid-token-flow#task#8.

Cause 2 of 7: CreateIdentityPool refused for an invalid pool name. AWS: `x-amzn-errortype: ValidationException:`. Emulator: missing. Traces: cognito-identity-create-identity-pool-invalid-name-rejected#task#1 (2 steps).

Cause 3 of 7: ListIdentities refused (invalid input). AWS: errortype header present. Emulator: missing. Traces: cogid-login-list-identities-next-token-pagination#task#11 (4 steps).

Cause 4 of 7: ListIdentityPools refused (invalid input). AWS: errortype header present. Emulator: missing. Traces: cogid-login-list-identity-pools-next-token-pagination#task#5 (3 steps).

Cause 5 of 7: GetCredentialsForIdentity refused. AWS: errortype header present. Emulator: missing. Traces: cogid-merge-getid-blank-absorbed-by-registered-developer-login#task#7 (3 steps).

Cause 6 of 7: GetOpenIdTokenForDeveloperIdentity refused for an out-of-range TokenDuration. AWS: errortype header present. Emulator: missing. Traces: cogid-merge-token-duration-above-max-rejected#task#3 (2 steps).

Cause 7 of 7: GetOpenIdToken refused. AWS: errortype header present. Emulator: missing. Traces: cognito-identity-get-openid-token-error-lifecycle#task#4.

(The same missing header shows up inside the status-mapping and message bundles filed alongside this one; those issues list it as a secondary symptom, this issue is the root cause.)

## Evidence
Replay of simparity-wire-aws-v1 — clusters whose ONLY divergent key is the response header:
- GetId: $headers.x-amzn-errortype expected 'ValidationException:', got None (cognito-identity-basic-identity-and-openid-token-flow#task#8).
- CreateIdentityPool: expected 'ValidationException:', got None (cognito-identity-create-identity-pool-invalid-name-rejected#task#1, 2 steps).
- ListIdentities: expected <shaped-not-asserted>, got None (cogid-login-list-identities-next-token-pagination#task#11, 4 steps).
- ListIdentityPools: expected <shaped-not-asserted>, got None (cogid-login-list-identity-pools-next-token-pagination#task#5, 3 steps).
- GetCredentialsForIdentity: expected <shaped-not-asserted>, got None (cogid-merge-getid-blank-absorbed-by-registered-developer-login#task#7, 3 steps).
- GetOpenIdTokenForDeveloperIdentity: expected <shaped-not-asserted>, got None (cogid-merge-token-duration-above-max-rejected#task#3, 2 steps).
- GetOpenIdToken: expected <shaped-not-asserted>, got None (cognito-identity-get-openid-token-error-lifecycle#task#4).
In all seven the status and body matched; only the header is absent.

## Steps to Reproduce
1. Replay cognito-identity-basic-identity-and-openid-token-flow step task#8 (GetId with invalid input) and inspect the response headers: no x-amzn-errortype is sent, while the recording carries 'ValidationException:'.
2. Repeat for each fixture#step under Traces (CreateIdentityPool, ListIdentities, ListIdentityPools, GetCredentialsForIdentity, GetOpenIdTokenForDeveloperIdentity, GetOpenIdToken error paths).
3. Any error response from this service is enough: the header is never emitted.
