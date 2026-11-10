Issue-Source: trace-sweep-divergence
Issue-Trace: cogid-merge-idp-proven-identity-login-conflicts
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cogid-merge-idp-proven-identity-login-conflicts
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|GetOpenIdTokenForDeveloperIdentity|4xx->4xx|$headers.x-amzn-errortype
Issue-Related: 20261030T212445Z-cognito-identity-developer-identity-token-and-credential-refusals.md
Issue-Related: 20261031T043446Z-cognito-identity-getopenidtokenfordeveloperidentity-answers-notauthorizedexception-instead-of-resourceconflictexception.md
Issue-Commit: 548748d6f2759d93814ba776da02695fcfeeb7d4

# cognito-identity: an already-linked public-provider login is refused as a logins mismatch instead of a link conflict

> _Note: this issue was identified against the code base at commit `548748d6f2759d93814ba776da02695fcfeeb7d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOpenIdTokenForDeveloperIdentity

## Description
One root cause (Cause 1 of 1).

Cause 1 of 1: GetOpenIdTokenForDeveloperIdentity mis-classifies a link conflict as an authorization failure. Fixture cogid-merge-idp-proven-identity-login-conflicts, step task#12.

AWS: GetOpenIdTokenForDeveloperIdentity implicitly LINKS the developer-provider user named in Logins to the identity proven by the other (public-provider) Logins entries. When one of those public-provider logins is already linked to a DIFFERENT identity in the same identity pool, AWS refuses the implicit link as a conflict between two existing identities, not as a bad credential. Smithy (com.amazonaws.cognitoidentity#AWSCognitoIdentityService): GetOpenIdTokenForDeveloperIdentity models both NotAuthorizedException (@httpError 403) and ResourceConflictException (@httpError 409); ResourceConflictException is documented as thrown when a user tries to use a login which is already linked to another account, which is exactly this state. The recorded response for this refusal is HTTP 400 with __type 'ResourceConflictException' and message 'Cannot merge these identities.' — take the status (400) and the code from the recording rather than the model's defaulted @httpError. That message is entirely FIXED text: it has no variable parts at all — it does not name the provider, the login key, the login value, or either identity id, so nothing in it is drawn from the request or from emulator state. The x-amzn-errortype header carries the same code, ResourceConflictException.

Emulator: the same request is refused with HTTP 400 but __type 'NotAuthorizedException' and message "Logins don't match. Please include at least one valid login for this identity or identity pool." (x-amzn-errortype 'NotAuthorizedException:'). The status already matches; the error code, the x-amzn-errortype header and the message all diverge. The emulator's logins-match authorization check fires before any link-conflict detection, so the condition 'this login already belongs to another identity' is never recognised as a conflict — it is reported as if the caller had presented a login that does not belong to the target identity.

Why: ResourceConflictException is the signal a client branches on to run conflict resolution — choose a surviving identity, call MergeDeveloperIdentities, or ask the end user to confirm that an already-linked account should be reused. NotAuthorizedException reads as a credential or permission failure, which clients treat as 'retry with better credentials' or surface as a sign-in error. So the conflict-resolution path never runs, the caller retries a state condition no retry can clear, and the distinct error code AWS models for precisely this case is unreachable from the emulator.

Fix: the implicit-link path of the GetOpenIdTokenForDeveloperIdentity handler — before the logins-match authorization check, detect that a supplied public-provider login is already bound to a different identity in the pool and raise the link-conflict error (code ResourceConflictException, fixed message 'Cannot merge these identities.', with x-amzn-errortype naming ResourceConflictException) instead of the logins-mismatch refusal. The ordering of the two checks is the defect, not the text of either one.

Traces: cogid-merge-idp-proven-identity-login-conflicts#task#12 (1 step in the cluster; replay reports no owning issue for this cluster key).

Related open issues that the replay does NOT report as carrying this cluster's replay key: 20261031T043446Z-cognito-identity-getopenidtokenfordeveloperidentity-answers-notauthorizedexception-instead-of-resourceconflictexception.md (describes the same mis-classification but the replay's ownership check does not bind it to this cluster — its recorded key covers only $.message, while this cluster also diverges on $.__type and $headers.x-amzn-errortype; a fixer should close both together), 20261030T212445Z-cognito-identity-developer-identity-token-and-credential-refusals.md and 20261029T173827Z-cognito-identity-10-net-new-developer-identity-root.md (other developer-identity refusal divergences, different keys).

## Evidence
Fixture cogid-merge-idp-proven-identity-login-conflicts, step task#12, cognito-identity GetOpenIdTokenForDeveloperIdentity, expected status 400 -> actual status 400:
  $.__type: value_mismatch — expected 'ResourceConflictException', got 'NotAuthorizedException'
  $.message: value_mismatch — expected 'Cannot merge these identities.', got "Logins don't match. Please include at least one valid login for this identity or identity pool."
  $headers.x-amzn-errortype: shape_mismatch — got 'NotAuthorizedException:' where the code must name ResourceConflictException

Message anatomy: the expected message 'Cannot merge these identities.' is wholly fixed text; there are no request-derived or state-derived substitutions in it. The emulator's substituted message is the generic logins-mismatch text, which belongs to a different error class.

Cluster: divergence_key service=cognito-identity|GetOpenIdTokenForDeveloperIdentity|status=400->400|keys=$.__type,$.message,$headers.x-amzn-errortype; affected_count 1; stub_path empty; owned_by empty.

Smithy support: GetOpenIdTokenForDeveloperIdentity errors include ResourceConflictException (client, @httpError 409) and NotAuthorizedException (client, @httpError 403); the recorded AWS refusal for this case is 400/ResourceConflictException.

## Steps to Reproduce
1. git_clone(), then replay_wire_corpus() against the emulator built from that checkout.
2. Re-replay fixture cogid-merge-idp-proven-identity-login-conflicts and inspect step task#12.
3. By hand, against one identity pool that has both a developer provider and a public (Cognito user pool) provider configured:
   a. Establish identity A linked to public-provider login L (e.g. via GetId/GetOpenIdToken with that login).
   b. Call GetOpenIdTokenForDeveloperIdentity with Logins containing the developer-provider key mapped to a DIFFERENT developer user identifier, plus the public-provider login L that is already bound to identity A.
4. Expected: HTTP 400, body __type 'ResourceConflictException', message exactly 'Cannot merge these identities.', response header x-amzn-errortype naming ResourceConflictException.
   Actual: HTTP 400 with __type 'NotAuthorizedException', message "Logins don't match. Please include at least one valid login for this identity or identity pool." and x-amzn-errortype 'NotAuthorizedException:'.
