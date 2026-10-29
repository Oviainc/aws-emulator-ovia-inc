Issue-Source: trace-sweep-divergence
Issue-Trace: cogid-login-delete-identities-batch-bounds
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cogid-login-delete-identities-batch-bounds cognito-identity-basic-identity-and-openid-token-flow cognito-identity-unauthenticated-guest-identity-flow cognito-identity-get-credentials-for-identity-errors cognito-identity-get-openid-token-error-lifecycle cognito-identity-get-openid-token-lifecycle cognito-identity-unlink-identity-login-management cogid-login-hide-disabled-after-unlink-and-relink cogid-login-user-pool-token-link-and-unlink cogid-merge-losing-identity-disabled-after-merge cogid-merge-token-duration-above-max-rejected cogid-merge-token-duration-zero-rejected cognito-identity-developer-authenticated-identity-merge-and-unlink cognito-identity-developer-authenticated-identity-flow cognito-identity-merge-developer-identities-workflow cogid-merge-same-public-provider-different-users-conflict
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|DescribeIdentity|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|DescribeIdentity|4xx->4xx|status
Issue-Key: cognito-identity|GetCredentialsForIdentity|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|GetCredentialsForIdentity|4xx->4xx|status
Issue-Key: cognito-identity|GetOpenIdToken|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|GetOpenIdToken|4xx->4xx|status
Issue-Key: cognito-identity|GetOpenIdToken|4xx->4xx|$.message
Issue-Key: cognito-identity|UnlinkIdentity|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|UnlinkIdentity|4xx->4xx|status
Issue-Key: cognito-identity|LookupDeveloperIdentity|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|LookupDeveloperIdentity|4xx->4xx|status
Issue-Key: cognito-identity|UnlinkDeveloperIdentity|2xx->4xx|$
Issue-Key: cognito-identity|UnlinkDeveloperIdentity|2xx->4xx|status
Issue-Key: cognito-identity|MergeDeveloperIdentities|4xx->4xx|$.__type
Issue-Key: cognito-identity|MergeDeveloperIdentities|4xx->4xx|$.message
Issue-Key: cognito-identity|MergeDeveloperIdentities|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|MergeDeveloperIdentities|4xx->4xx|status
Issue-Commit: 2019710a3c4752758402c7d5e42a18d73361dfac

# cognito-identity: identity-scoped refusals answer 403/404/409 instead of HTTP 400

> _Note: this issue was identified against the code base at commit `2019710a3c4752758402c7d5e42a18d73361dfac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeIdentity

## Description
This issue bundles 10 root causes on the identity-scoped (data-plane) operations. The emulator serves each modeled client error with the status from its Smithy @httpError trait — ResourceNotFoundException 404, NotAuthorizedException 403, ResourceConflictException 409 — while the recorded AWS responses all carry HTTP 400 and put the error class only in the awsJson1_1 body `__type` plus the `x-amzn-errortype` header. Smithy (this service): ResourceNotFoundException @httpError(404), NotAuthorizedException @httpError(403), ResourceConflictException @httpError(409); all three are modeled errors of DescribeIdentity, GetCredentialsForIdentity, GetOpenIdToken, UnlinkIdentity, UnlinkDeveloperIdentity, LookupDeveloperIdentity and MergeDeveloperIdentities. Recorded behavior shows 400 for all of them, so 400 is the requirement on the wire. Two causes additionally carry a wrong error class/message, noted inline. Fix: the awsJson1_1 error status mapping for this service (shared serializer), plus — for causes 4, 9 and 10 — the error selection in the named operation's handler.

Cause 1 of 10: DescribeIdentity for an identity id that does not exist. AWS: 400 + `__type` ResourceNotFoundException. Emulator: 404, and no `x-amzn-errortype` header. Why: clients branching on status see a transport 404, not the modeled error. Fix: error serializer status mapping. Traces: cogid-login-delete-identities-batch-bounds#task#8 (4 steps).

Cause 2 of 10: GetCredentialsForIdentity for an unknown identity id. AWS: 400 + ResourceNotFoundException (header `x-amzn-errortype: ResourceNotFoundException:`). Emulator: 404, header absent. Fix: as cause 1. Traces: cognito-identity-get-credentials-for-identity-errors#task#4.

Cause 3 of 10: GetOpenIdToken for an unknown identity id. AWS: 400 + ResourceNotFoundException. Emulator: 404. Fix: as cause 1. Traces: cognito-identity-get-openid-token-error-lifecycle#task#5 (2 steps).

Cause 4 of 10: GetOpenIdToken naming an identity whose identity POOL has been deleted. AWS: 400 and the message names the missing POOL — fixed text "IdentityPool '" + the pool id derived from the request + "' not found." Emulator: 404 and the message names the identity instead — "Identity '<identity-id>' not found." — so both the status and the subject of the message are wrong. Why: a client cannot tell which resource vanished. Fix: status mapping + the not-found lookup order in the GetOpenIdToken handler (resolve the pool before the identity). Traces: cognito-identity-get-openid-token-error-lifecycle#task#9.

Cause 5 of 10: UnlinkIdentity for an unknown identity. AWS: 400 + ResourceNotFoundException. Emulator: 404. Fix: as cause 1. Traces: cognito-identity-unlink-identity-login-management#task#5.

Cause 6 of 10: UnlinkIdentity refused as unauthorized (logins do not match the identity). AWS: 400 + `__type` NotAuthorizedException. Emulator: 403. Fix: as cause 1. Traces: cogid-login-hide-disabled-after-unlink-and-relink#task#15 (2 steps).

Cause 7 of 10: LookupDeveloperIdentity refused as unauthorized. AWS: 400 + NotAuthorizedException. Emulator: 403. Fix: as cause 1. Traces: cogid-merge-losing-identity-disabled-after-merge#task#9 (4 steps).

Cause 8 of 10: UnlinkDeveloperIdentity on a developer login that IS linked to the identity. AWS: HTTP 200 with an empty body (operation output is Unit). Emulator: 403 with {"__type":"NotAuthorizedException","message":"You do not have access to this identity"} — it refuses a call AWS accepts. Why: the unlink never happens, so every later assertion about the identity's Logins is wrong too. Fix: the UnlinkDeveloperIdentity handler's authorization/ownership check (it must accept the identity's own developer login), and the status mapping for the error it does raise. Traces: cognito-identity-developer-authenticated-identity-flow#task#11.

Cause 9 of 10: MergeDeveloperIdentities naming a developer user identifier that was never registered. AWS: 400, `__type` ResourceNotFoundException, message fixed text "User '" + the DeveloperUserIdentifier from the request + "' not found.", header `x-amzn-errortype: ResourceNotFoundException:`. Emulator: 403 with `__type` NotAuthorizedException and message 'You do not have access to this identity' — wrong status, wrong error code and wrong message. Why: a client cannot distinguish 'unknown user' from 'not permitted'. Fix: the MergeDeveloperIdentities handler's error selection (missing source user => ResourceNotFoundException) plus status mapping. Traces: cognito-identity-developer-authenticated-identity-merge-and-unlink#task#11 (2 steps).

Cause 10 of 10: MergeDeveloperIdentities where both identities are linked to the SAME public provider as two different users. AWS: 400 with the fixed message text 'Cannot merge these identities.' Emulator: 409 and a re-worded message that embeds the provider and a verdict — "The users to be merged are associated with the provider '<provider-name>' as two different users." Why: the status and message are both requirements; a client matching on AWS's text never matches. Fix: MergeDeveloperIdentities handler message text + status mapping. Traces: cogid-merge-same-public-provider-different-users-conflict#task#13.

Related open issue that does not cover these (no replay key, different fault): '20261029T173827Z-cognito-identity-10-net-new-developer-identity-root.md'.

## Evidence
Replay of simparity-wire-aws-v1, expected vs actual per cluster:
- DescribeIdentity: expected 400, got 404; x-amzn-errortype missing (cogid-login-delete-identities-batch-bounds#task#8, 4 steps).
- GetCredentialsForIdentity: expected 400, got 404; errortype header missing (cognito-identity-get-credentials-for-identity-errors#task#4).
- GetOpenIdToken: expected 400, got 404 (cognito-identity-get-openid-token-error-lifecycle#task#5, 2 steps).
- GetOpenIdToken: expected 400, got 404; $.message expected "IdentityPool 'us-west-2:<pool-id>' not found.", got "Identity 'us-west-2:<identity-id>' not found." (cognito-identity-get-openid-token-error-lifecycle#task#9).
- UnlinkIdentity: expected 400, got 404 (cognito-identity-unlink-identity-login-management#task#5).
- UnlinkIdentity: expected 400, got 403 (cogid-login-hide-disabled-after-unlink-and-relink#task#15, 2 steps).
- LookupDeveloperIdentity: expected 400, got 403 (cogid-merge-losing-identity-disabled-after-merge#task#9, 4 steps).
- UnlinkDeveloperIdentity: expected 200 with body {}, got 403 with {'__type': 'NotAuthorizedException', 'message': 'You do not have access to this identity'} (cognito-identity-developer-authenticated-identity-flow#task#11).
- MergeDeveloperIdentities: expected 400 / ResourceNotFoundException / "User 'never-registered-user' not found.", got 403 / NotAuthorizedException / 'You do not have access to this identity' (cognito-identity-developer-authenticated-identity-merge-and-unlink#task#11, 2 steps).
- MergeDeveloperIdentities: expected 400 / 'Cannot merge these identities.', got 409 / "The users to be merged are associated with the provider '<provider>' as two different users." (cogid-merge-same-public-provider-different-users-conflict#task#13).
Smithy: ResourceNotFoundException httpError 404, NotAuthorizedException httpError 403, ResourceConflictException httpError 409; protocol awsJson1_1; recorded AWS status for all three is 400.

## Steps to Reproduce
1. Replay cogid-login-delete-identities-batch-bounds step task#8: DescribeIdentity with a deleted identity id -> 404 instead of 400.
2. Replay each other cause's fixture#step from Traces: GetCredentialsForIdentity/GetOpenIdToken/UnlinkIdentity on unknown identities; UnlinkIdentity and LookupDeveloperIdentity with non-matching logins; UnlinkDeveloperIdentity on a linked developer login (expect 200); MergeDeveloperIdentities with an unregistered DeveloperUserIdentifier and with two different users of the same public provider.
3. Compare HTTP status, body __type, message and x-amzn-errortype against the recorded responses.
