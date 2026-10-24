Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.list-tags-for-resource-unauthorized-rejected cognito-identity.a-token-minted-by-another-pool-is-refused-and-vends-nothing cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-governs-tenant-access cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-without-tag-session-rejected cognito-identity.lookup-developer-identity-cross-pool-isolation cognito-identity.concurrent-merge-developer-identities-same-source-serializes cognito-identity.merge-developer-identities-concurrent-requests-serialized-consistently cognito-identity.developer-authenticated-credentials-write-and-read-dynamodb cognito-identity.developer-identity-vends-authenticated-role-credentials cognito-identity.tag-resource-key-exceeding-length-rejected
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.list-tags-for-resource-unauthorized-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: cognito-identity.a-token-minted-by-another-pool-is-refused-and-vends-nothing|run#16|AssertionError|assert code == EXPECTED_CODE, (
Issue-Key: cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-governs-tenant-access|_stack_outputs#10|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-without-tag-session-rejected|_stack_outputs#11|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cognito-identity.lookup-developer-identity-cross-pool-isolation|run#9|AssertionError|assert code == "NotAuthorizedException", (
Issue-Key: cognito-identity.concurrent-merge-developer-identities-same-source-serializes|run#17|AssertionError|assert successful_labels, f"neither concurrent merge call succeeded: {outcomes}"
Issue-Key: cognito-identity.merge-developer-identities-concurrent-requests-serialized-consistently|run#15|AssertionError|assert len(oks) >= 1, f"neither concurrent merge call succeeded: {results!r}"
Issue-Key: cognito-identity.developer-authenticated-credentials-write-and-read-dynamodb|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: cognito-identity.developer-identity-vends-authenticated-role-credentials|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: cognito-identity.tag-resource-key-exceeding-length-rejected|run#11|AssertionError|assert message == (
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# cognito-identity: ListTagsForResource ignores IAM authorization, a foreign-pool token answers UnknownService, CFN stack Outputs carry no OutputValue, developer-identity paths are unimplemented or never converge (6 causes)

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
This issue bundles SIX distinct root causes reached through cognito-identity behavioral checks. All six must be fixed and re-replayed (see trace_ids) before closing.

=== Cause 1 of 6: ListTagsForResource performs no IAM authorization — a zero-permission principal reads a pool's tags ===
AWS: every cognito-identity control-plane operation is IAM-authorized. Smithy (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice): ListTagsForResource input {ResourceArn required, @length 20..2048}, errors [InvalidParameterException 400, NotAuthorizedException 403, ResourceNotFoundException 404, TooManyRequestsException 429, InternalErrorException 500]. The IAM denial itself is the framework-level AccessDeniedException with HTTP 403, and its message is built from the request: "User: <caller arn> is not authorized to perform: cognito-identity:ListTagsForResource on resource: <pool arn> because no identity-based policy allows the cognito-identity:ListTagsForResource action" (recorded). Granting exactly that action on exactly that resource turns the identical call into a success, which is how the check proves the refusal is the authorization boundary and not a bad request.
Emulator: the brand-new IAM user with zero attached policies gets HTTP 200 and the tag set back.
Why: a client relying on AWS's behavior believes its least-privilege policy is enforced; against the emulator every denied-access/least-privilege assertion about Cognito pool tags passes, and the failure only appears in production. It is also a confidentiality gap: pool tags are readable by any caller.
Fix: credential-to-principal resolution in the request-auth layer plus an authorization check in the ListTagsForResource handler of the cognito-identity service's ops/ (list_tags_for_resource.rs).
Related but NOT covering: 20261023T185902Z-cognito-identity-input-validation-and-error-classification-diverge.md Cause 5 describes the same missing per-caller authorization for GetIdentityPoolRoles/DeleteIdentityPool but does not carry this check; the ListTagsForResource scaffold-stub issue (fixed) only made the operation answer at all.
Traces: cognito-identity.list-tags-for-resource-unauthorized-rejected.

=== Cause 2 of 6: a login token minted by a DIFFERENT identity pool is answered UnknownService instead of being refused NotAuthorizedException ===
AWS: GetCredentialsForIdentity is anonymous-capable — Smithy binds it with an empty auth list and optionalAuth:true — and models NotAuthorizedException (HTTP 403). Recorded behavior: presenting a token minted by another identity pool is refused with NotAuthorizedException (recorded message example: "Invalid login token. Not a Cognito token.") and nothing is vended; token binding is checked before the pool's role configuration is consulted.
Emulator: the call is answered with error code UnknownService, message "no emulated service is registered for 'awscognitoidentityservice'" (the Smithy service id), i.e. the request never reaches the cognito-identity service even though it answers signed calls in this same run.
Why: a client gets an error code no SDK models and cannot distinguish a forged/foreign token from an absent service, and the cross-pool token boundary cannot be verified at all.
Fix: the request router / auth layer must resolve the target service for anonymous (optionalAuth / empty-auth) requests, and the GetCredentialsForIdentity handler must then apply the pool-binding check on each Logins token.
Related but NOT covering: 20261023T185902Z-cognito-identity-unsigned-anonymous-requests-answer-unknownservice-and.md Cause 1 reports the same routing cause for GetId checks; this check's replay key is not on it.
Traces: cognito-identity.a-token-minted-by-another-pool-is-refused-and-vends-nothing.

=== Cause 3 of 6: CloudFormation DescribeStacks returns Outputs entries with no OutputValue, so a CFN-deployed principal-tag scenario cannot read any output ===
AWS: DescribeStacks returns every stack Output as a pair of OutputKey and OutputValue, the value being the resolved !Ref/!GetAtt of the resource (for AWS::Cognito::IdentityPool the '<region>:<guid>' pool id; for AWS::Cognito::IdentityPoolPrincipalTag the compound '<pool id>|<provider name>' id) (recorded, us-west-2 2026-10-01).
Emulator: each Outputs element lacks the OutputValue member entirely, so the standard {o["OutputKey"]: o["OutputValue"]} read raises KeyError: 'OutputValue' and the scenario dies immediately after the stack reports success.
Why: any client that deploys infrastructure with CloudFormation and reads back ids through stack Outputs is broken even though CreateStack reports success — the whole infrastructure-as-code path for Cognito principal tags is a dead end.
Fix: the CloudFormation DescribeStacks output serializer (and the stack-output resolution behind it) must populate OutputValue from the resource's physical id.
Note on progress: 20261023T185902Z-cloudformation-cognito-identity-sts-cloudformation-aws-cognito-identitypoolprincipaltag-is-rejected-as-an.md (fixed) made CreateStack accept AWS::Cognito::IdentityPoolPrincipalTag — these two checks now get past CreateStack and fail one step later, at the Outputs read, so this is not a refusal of that fixed operation.
Related but NOT covering: 20261024T065715Z-cloudformation-cognito-identity-sts-cloudformation-stack-outputs-are-returned-without-outputvalue-so.md and 20261023T114649Z-cloudformation-outputs-carry-no-outputvalue-failed-resources-never.md describe the same serializer defect but carry neither of these two checks' replay keys.
Traces: cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-governs-tenant-access, cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-without-tag-session-rejected.

=== Cause 4 of 6: MergeDeveloperIdentities and LookupDeveloperIdentity answer NotImplemented, so developer-identity linking and its isolation/conflict semantics are unreachable ===
AWS: Smithy — MergeDeveloperIdentities (SourceUserIdentifier, DestinationUserIdentifier, DeveloperProviderName, IdentityPoolId all required) returns IdentityId and models [InvalidParameterException 400, NotAuthorizedException 403, ResourceConflictException 409, ResourceNotFoundException 404, TooManyRequestsException 429]; LookupDeveloperIdentity carries the same error set. Recorded behavior: looking up an identity that belongs to a DIFFERENT pool is refused with NotAuthorizedException (403); two concurrent merges of the same source serialize so that at least one call succeeds and the source settles on exactly one destination.
Emulator: every one of these calls answers error code NotImplemented, so each check fails on the absence of the operation rather than on its semantics.
Why: developer-authenticated identity linking is unavailable, and the conflict (409) and cross-pool authorization (403) branches a client implements are dead code against the emulator.
Fix: implement the handlers services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/merge_developer_identities.rs and .../lookup_developer_identity.rs, including pool-scoping of lookups and serialization of concurrent merges.
Note: the suite auto-files scaffold-stub issues naming those two operations and files (sibling checks are marked ALREADY FILED against them); these three checks were NOT marked, so they are recorded here purely so the fixer re-replays them after the stubs are implemented.
Traces: cognito-identity.lookup-developer-identity-cross-pool-isolation, cognito-identity.concurrent-merge-developer-identities-same-source-serializes, cognito-identity.merge-developer-identities-concurrent-requests-serialized-consistently.

=== Cause 5 of 6: the developer-identity credential-vend flow never converges — both vend checks hang to the 60 s harness cap ===
AWS: GetOpenIdTokenForDeveloperIdentity (Logins keyed by the developer provider) followed by GetCredentialsForIdentity with the Logins key cognito-identity.amazonaws.com vends a session in seconds; the response carries Credentials with the secret under SecretKey and the vended principal is named CognitoIdentityCredentials (recorded, us-west-2 2026-09-10). Smithy: GetCredentialsForIdentity output {IdentityId, Credentials}, errors include NotAuthorizedException 403 and InvalidIdentityPoolConfigurationException 400.
Emulator: neither check ever reaches its assertions — both are killed at the harness's 60 s cap while polling for the vend to succeed (the checks' own propagation window is 180 s, so a converging emulator would have answered long before).
Hypothesis: the vend leg is answered with a refusal that the checks' poll helper counts as "not yet", so it loops forever; the most likely source is the unsigned-routing defect of Cause 2 above, because GetCredentialsForIdentity is optionalAuth and is therefore sent unsigned.
Why: a client cannot distinguish "credentials not propagated yet" from "credentials will never be vended"; the whole developer-authenticated credential path appears to hang rather than to fail.
Fix: the GetCredentialsForIdentity path in the cognito-identity service — anonymous request routing, resolution of the authenticated role for a developer-provider login, and the credentials vend itself.
Traces: cognito-identity.developer-authenticated-credentials-write-and-read-dynamodb, cognito-identity.developer-identity-vends-authenticated-role-credentials.

=== Cause 6 of 6: the ValidationException message for a map-key length violation is not the message AWS builds ===
AWS: TagResource.Tags targets IdentityPoolTagsType, whose key TagKeysType carries @length 1..128 (Smithy). A violation is refused with HTTP 400 ValidationException whose message is built from the request and the model in AWS's standard construction: the rendered VALUE of the offending member, the member path, and the violated constraints listed per member — recorded: "1 validation error detected: Value '{<129-char key>=some-value, valid-key=some-other-value}' at 'tags' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]". Because that construction is derived from the request and the modeled constraint, it is a requirement, not a recorded example.
Emulator: the status and the code are correct, but the message renders only the offending key instead of the whole map and collapses the two member constraints into one clause: "1 validation error detected: Value 'kkkk…(129)' at 'tags' failed to satisfy constraint: Map keys must have length greater than or equal to 1 and less than or equal to 128".
Why: clients and test harnesses that parse AWS's validation construction (member path plus the per-member constraint list) read a different value and a different constraint rendering for the same request.
Fix: the shared constraint-validation message builder used by the cognito-identity request-validation path — render the member's value as the request presented it and emit map-key/value constraints as AWS's per-member constraint list.
Related but NOT covering: 20261023T185902Z-cognito-identity-input-validation-and-error-classification-diverge.md Cause 1 covers the error-code classification and the message prefix for scalar members; it carries this check id but was filed when the operation still answered NotImplemented, and it does not describe the map-key rendering.
Traces: cognito-identity.tag-resource-key-exceeding-length-rejected.

## Evidence
Cause 1 — cognito-identity.list-tags-for-resource-unauthorized-rejected: a pool is created and tagged by the owner, then a brand-new IAM user with zero attached policies calls ListTagsForResource on the pool ARN.
  expected: HTTP 403, Code 'AccessDeniedException', Message "User: <user arn> is not authorized to perform: cognito-identity:ListTagsForResource on resource: <pool arn> because no identity-based policy allows the cognito-identity:ListTagsForResource action"
  actual:   AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'Tags': {'Environment': 'test'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Cause 2 — cognito-identity.a-token-minted-by-another-pool-is-refused-and-vends-nothing:
  AssertionError: expected 'NotAuthorizedException' for a foreign-pool token, got 'UnknownService': "no emulated service is registered for 'awscognitoidentityservice'"
  Model: smithy_query operation=GetCredentialsForIdentity -> "auth": [], "optionalAuth": true; errors include NotAuthorizedException (403).

Cause 3 — cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-governs-tenant-access and ...-without-tag-session-rejected, both at the stack-outputs read:
  File "check_...py", line 360, in _stack_outputs
    return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
  KeyError: 'OutputValue'
  (the stack itself is reported created; only the Output's value member is absent)

Cause 4 — cognito-identity.lookup-developer-identity-cross-pool-isolation:
  AssertionError: expected cross-pool lookup of us-west-2:a5992784-faad-4247-b806-b39efee06c1b to be rejected with NotAuthorizedException, got NotImplemented
  cognito-identity.concurrent-merge-developer-identities-same-source-serializes:
  AssertionError: neither concurrent merge call succeeded: {'A': ('error', 'NotImplemented'), 'B': ('error', 'NotImplemented')}
  cognito-identity.merge-developer-identities-concurrent-requests-serialized-consistently:
  AssertionError: neither concurrent merge call succeeded: [('error', 'NotImplemented'), ('error', 'NotImplemented')]

Cause 5 — cognito-identity.developer-authenticated-credentials-write-and-read-dynamodb: "Check timed out after 60.0s"; cognito-identity.developer-identity-vends-authenticated-role-credentials: "Check timed out after 60.0s". Both checks poll the vend with a 180 s propagation budget and never see a success inside the harness cap; no response of any kind is asserted on.

Cause 6 — cognito-identity.tag-resource-key-exceeding-length-rejected (TagResource with Tags={'k'*129: 'some-value', 'valid-key': 'some-other-value'}):
  expected ValidationException + "1 validation error detected: Value '{kkk…=some-value, valid-key=some-other-value}' at 'tags' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]"
  actual   message: "1 validation error detected: Value 'kkk…(129 k's)' at 'tags' failed to satisfy constraint: Map keys must have length greater than or equal to 1 and less than or equal to 128" (code matched)
  Model: smithy_query shape=IdentityPoolTagsType -> key target TagKeysType @length{min:1,max:128}, value TagValueType @length{min:0,max:256}.

## Steps to Reproduce
Boot the emulator and replay the ten checks in trace_ids. Minimal reproductions:
1. Cause 1: CreateIdentityPool, TagResource(ResourceArn=<pool arn>, Tags={'Environment':'test'}); create an IAM user with no policies and sign ListTagsForResource(ResourceArn=<pool arn>) with its keys -> expect HTTP 403 AccessDeniedException naming the caller, action and resource; observe HTTP 200 with the tags. Then attach a policy allowing only cognito-identity:ListTagsForResource on that ARN and confirm the identical call succeeds.
2. Cause 2: create two identity pools; mint a token in pool A (GetOpenIdTokenForDeveloperIdentity) and call GetCredentialsForIdentity for an identity of pool B with that token in Logins -> expect HTTP 403 NotAuthorizedException and no Credentials; observe error code UnknownService "no emulated service is registered for 'awscognitoidentityservice'".
3. Cause 3: CreateStack with a template declaring AWS::Cognito::IdentityPool plus AWS::Cognito::IdentityPoolPrincipalTag and an Outputs block built from !Ref; wait for CREATE_COMPLETE; DescribeStacks and read Stacks[0].Outputs -> expect each entry to carry OutputKey and OutputValue; observe entries with no OutputValue member.
4. Cause 4: LookupDeveloperIdentity(IdentityPoolId=<pool B>, IdentityId=<identity of pool A>) -> expect 403 NotAuthorizedException; observe NotImplemented. Issue two concurrent MergeDeveloperIdentities for the same source -> expect at least one success; observe NotImplemented for both.
5. Cause 5: GetOpenIdTokenForDeveloperIdentity for a developer provider, then GetCredentialsForIdentity with Logins={'cognito-identity.amazonaws.com': <token>} -> expect Credentials (secret under SecretKey) within seconds; observe the flow never completing inside 60 s.
6. Cause 6: TagResource(ResourceArn=<pool arn>, Tags={'k'*129: 'some-value', 'valid-key': 'some-other-value'}) -> expect 400 ValidationException with AWS's construction (whole-map value rendering, per-member constraint list); observe the shortened value rendering and collapsed constraint clause.
