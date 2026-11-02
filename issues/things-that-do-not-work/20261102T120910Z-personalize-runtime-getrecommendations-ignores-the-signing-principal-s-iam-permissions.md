Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: personalize-runtime.get-recommendations-missing-iam-permission-denied
Issue-Shape-Id: com.amazonaws.personalizeruntime#AmazonPersonalizeRuntime
Issue-Key: personalize-runtime.get-recommendations-missing-iam-permission-denied|run#17|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Commit: 8a15654a3e32a09451431e59a712f0f10794d652

# GetRecommendations ignores the signing principal's IAM permissions: a role without personalize:GetRecommendations gets InvalidInputException instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `8a15654a3e32a09451431e59a712f0f10794d652`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRecommendations

## Description
NOTE: the open issue 20261023T071630Z-personalize-runtime-never-denies-an-unauthorized-caller-cross.md describes this same authorization gap as its "Cause 2 of 2" and even lists this check id in its Issue-Trace-Ids, but the replay report did NOT mark this check as OWNED by it (only the cross-account sibling check was marked owned), so it is filed here rather than dropped. A fixer should close it together with that issue if they share one fix.

Cause 1 of 1: the personalize-runtime data plane performs no identity authorization at all, so the signing principal's IAM policy is never consulted before the campaign lookup.

AWS: credentials obtained by assuming a role whose only identity-based statement allows `personalize:DescribeCampaign` cannot call GetRecommendations. The call is refused at the authorization layer, before any resource lookup, with HTTP 403 and error code `AccessDeniedException`. The message is AWS's standard identity-based-denial template: fixed text `User: <principal> is not authorized to perform: personalize:GetRecommendations on resource: <resource> because no identity-based policy allows the personalize:GetRecommendations action`, where `<principal>` is the assumed-role session ARN derived from the signing credentials (`arn:aws:sts::<account>:assumed-role/<role-name>/<session-name>`) and `<resource>` is the `campaignArn` taken from the request; the action name is fixed for this operation. The contrast that proves the refusal is an authz-layer one: the identical call made with unrestricted ambient credentials reaches business logic and returns HTTP 400 `InvalidInputException` with the fixed-template message `<campaignArn> does not exist or is not active yet.` (the ARN is the request value). Smithy: GetRecommendations models only InvalidInputException (400) and ResourceNotFoundException (404) — AccessDeniedException is the service-framework authorization refusal, not a modeled operation error, so the 403/AccessDeniedException pairing is recorded behavior only.

Emulator: STS AssumeRole issues session credentials, the restricted client signs GetRecommendations with them, and the emulator answers exactly as it does for the unrestricted caller — HTTP 400 `InvalidInputException`, `"<campaignArn> does not exist or is not active yet."`. No IAM policy evaluation happens for the data-plane action; authorized and unauthorized callers are indistinguishable.

Why: a client that branches on AccessDeniedException (surface an IAM/resource-policy misconfiguration to the operator) versus InvalidInputException (treat the campaign as not yet ACTIVE and keep polling) takes the wrong branch and retries forever against a call it will never be permitted to make. Worse, any least-privilege or negative-permission test written against the emulator passes vacuously while the same deployment is denied by real AWS.

Fix: an identity-authorization stage ahead of the personalize-runtime handlers — shared request middleware that resolves the SigV4 principal (including assumed-role sessions minted by STS), evaluates the IAM service's role/identity policies for the `personalize:GetRecommendations` (and sibling `personalize:GetPersonalizedRanking` / `personalize:GetActionRecommendations`) action, and returns 403 AccessDeniedException with the standard template message before the campaign/recommender registry lookup runs. The operation handler and its ARN-resolution path must stay unreached once the stage refuses.

Traces: personalize-runtime.get-recommendations-missing-iam-permission-denied.

## Evidence
Failing check personalize-runtime.get-recommendations-missing-iam-permission-denied:
  AssertionError: expected AccessDeniedException, got InvalidInputException
  (check_personalize_runtime_get_recommendations_missing_iam_permission_denied.py line 67: `assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"`)

Expected (AWS): HTTP 403, Code=AccessDeniedException, Message="User: arn:aws:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: personalize:GetRecommendations on resource: <campaignArn> because no identity-based policy allows the personalize:GetRecommendations action" — fixed text as quoted, with principal ARN from the signing session credentials and resource from the request's campaignArn.
Actual (emulator): HTTP 400, Code=InvalidInputException, Message="<campaignArn> does not exist or is not active yet." — i.e. the same answer the unrestricted positive-control call gets, so the restricted session is not refused at all.

The check's positive control (the ambient, unrestricted credentials on the same nonexistent campaign ARN) legitimately yields 400 InvalidInputException, which confirms the emulator makes no distinction between an authorized and an unauthorized caller.
Smithy (com.amazonaws.personalizeruntime#GetRecommendations): errors = InvalidInputException (400), ResourceNotFoundException (404); no authorization error is modeled, so the 403/AccessDeniedException refusal is framework-level.

## Steps to Reproduce
1. Boot the emulator.
2. iam.CreateRole with a trust policy allowing sts:AssumeRole from the caller's account root.
3. iam.PutRolePolicy on that role with a single statement: Effect=Allow, Action="personalize:DescribeCampaign", Resource="*" (deliberately WITHOUT personalize:GetRecommendations).
4. sts.AssumeRole for that role and build a personalize-runtime client from the returned session credentials.
5. Call GetRecommendations(campaignArn="arn:aws:personalize:<region>:<own-account>:campaign/<nonexistent-name>", userId=<any>) with those restricted credentials.
   Observed: HTTP 400 InvalidInputException "<arn> does not exist or is not active yet."
   Expected: HTTP 403 AccessDeniedException with the identity-based-denial message naming the assumed-role session principal, the personalize:GetRecommendations action and the campaignArn as resource.
6. Repeat step 5 with the ambient (unrestricted) credentials: that call should still return HTTP 400 InvalidInputException — today both calls return the identical 400.
