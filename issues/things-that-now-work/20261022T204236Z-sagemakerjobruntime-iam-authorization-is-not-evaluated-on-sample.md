Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemakerjobruntime.sample-no-iam-permission-rejected sagemakerjobruntime.fine-grained-action-permission-rejected sagemakerjobruntime.sample-vs-stream-action-independence-rejected sagemakerjobruntime.cross-account-resource-condition-rejected
Issue-Shape-Id: com.amazonaws.sagemakerjobruntime#AgenticRFTRuntimeService
Issue-Key: sagemakerjobruntime.sample-no-iam-permission-rejected|run#9|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: sagemakerjobruntime.fine-grained-action-permission-rejected|run#9|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: sagemakerjobruntime.sample-vs-stream-action-independence-rejected|run#9|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: sagemakerjobruntime.cross-account-resource-condition-rejected|run#10|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Commit: f973f78f74c99aad4e85f2736f1fde89ed7327e3

# sagemakerjobruntime: IAM authorization is not evaluated on Sample/SampleWithResponseStream/CompleteRollout — restricted callers reach validation and get ValidationException/400 instead of AccessDeniedException/403

> _Note: this issue was identified against the code base at commit `f973f78f74c99aad4e85f2736f1fde89ed7327e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Sample

## Description
The sagemakerjobruntime data plane appears to perform NO identity-based authorization at all: every request, regardless of the policy attached to the assumed role it is signed with, falls straight through to the job/trajectory-lookup layer. Three distinct authorization behaviours are therefore missing. A fixer must close all THREE causes below before submitting.

Cause 1 of 3: Sample does not check that the caller is granted sagemaker:Sample at all.
  AWS: Sample is modeled with AccessDeniedException as both a per-operation and a service-common error (Smithy: AgenticRFTRuntimeService common_errors include AccessDeniedException; Sample.errors includes AccessDeniedException, httpError 403, doc "You do not have permission to perform this operation."). A principal whose only grant is sts:GetCallerIdentity is denied before any resource lookup: HTTP 403 with error code AccessDeniedException. The message is the standard IAM identity-based-denial string composed from the request — "User: <caller session ARN> is not authorized to perform: sagemaker:Sample on resource: <JobArn> because no identity-based policy allows the sagemaker:Sample action" — so the composition rule (principal ARN, action, resource ARN) is the requirement, not any one recorded string.
  Emulator: returns ValidationException (HTTP 400, "could not find job") for the unauthorized caller — i.e. the same answer as the fully-permissioned control call in the same check, which proves authorization is never consulted.
  Why: a client (or a security test) cannot distinguish "not permitted" from "job does not exist"; least-privilege policies appear to have no effect, and code that retries or surfaces 403 vs 400 differently behaves wrongly. A 400 is also non-retryable-but-validation-shaped, steering callers to fix their request instead of their policy.
  Fix: the authorization/credential-resolution layer in front of the sagemakerjobruntime operation handlers — resolve the signing principal, evaluate its identity-based policy for the operation's IAM action, and return AccessDeniedException/403 before the job-lookup validation in the Sample handler.
  Traces: sagemakerjobruntime.sample-no-iam-permission-rejected.

Cause 2 of 3: per-action granularity is not enforced — a grant for one sagemakerjobruntime action authorizes the others.
  AWS: each operation maps to its own IAM action (sagemaker:Sample, sagemaker:SampleWithResponseStream, sagemaker:CompleteRollout, sagemaker:UpdateReward). A role granted only sagemaker:CompleteRollout + sagemaker:UpdateReward is denied on Sample, and a role granted only sagemaker:Sample is denied on SampleWithResponseStream — AccessDeniedException/403 in both cases (Smithy: AccessDeniedException httpError 403 is a common error bound to all four operations), with the same request-composed "...is not authorized to perform: <action> on resource: <JobArn> because no identity-based policy allows the <action> action" message. The action a caller IS granted must clear authorization and reach the resource layer (CompleteRollout -> ResourceNotFoundException/404 for an unknown trajectory; Sample -> ValidationException/400 for an unknown job).
  Emulator: both ungranted calls return ValidationException/400 instead of AccessDeniedException/403, so the two inference operations behave as aliases of one permission and the action name in the policy is ignored.
  Why: policies that intentionally withhold streaming inference, or that grant only rollout bookkeeping, are silently over-permissive under the emulator; a tenant-isolation or least-privilege test passes here and fails against real AWS.
  Fix: the authorization layer must key the policy decision on the per-operation IAM action (one action per handler), not on a service-wide allow.
  Traces: sagemakerjobruntime.fine-grained-action-permission-rejected, sagemakerjobruntime.sample-vs-stream-action-independence-rejected.

Cause 3 of 3: resource-level conditions (aws:ResourceAccount / aws:PrincipalAccount) are not evaluated, so cross-account JobArns are accepted.
  AWS: when the grant for sagemaker:Sample on arn:aws:sagemaker:*:*:job/* carries Condition StringEquals aws:ResourceAccount = ${aws:PrincipalAccount} (the condition shape used by the AWS-managed job-runtime access policy), a call whose JobArn names a DIFFERENT 12-digit account fails the condition and is denied with AccessDeniedException/403 (message form "User: <caller session ARN> is not authorized to access this resource"), while the identical call against the caller's own account clears authorization and reaches job-lookup validation (ValidationException/400). Smithy: Sample.JobArn is an httpHeader X-Amzn-SageMaker-Job-Arn constrained by pattern ^arn:aws[a-z\\-]*:sagemaker:[a-z0-9\\-]*:[0-9]{12}:job/...$, i.e. the ARN carries the owning account the condition is evaluated against; AccessDeniedException httpError 403.
  Emulator: the cross-account JobArn is answered with ValidationException/400 (job not found) — the resource account is never compared against the principal's account, so authorization is not resource-scoped.
  Why: cross-tenant isolation is unverifiable and appears to hold for the wrong reason (the foreign job merely does not exist in local state); a caller relying on the resource-account condition to confine a role gets no enforcement.
  Fix: the authorization layer's policy evaluation must extract the resource account from the request's JobArn and evaluate resource/condition keys (aws:ResourceAccount against aws:PrincipalAccount) before the handler's job lookup.
  Traces: sagemakerjobruntime.cross-account-resource-condition-rejected.

Related but not covering this: no open issue mentions sagemakerjobruntime (list_existing_issues query='jobruntime' returns none); the open sagemaker issues concern control-plane stubs and ValidationException message text in a different service.

## Evidence
Behavioral suite: sagemakerjobruntime 10/14 (71%), 4 mismatches, all on the same assertion.

Expected (AWS) vs actual (emulator):
- sample-no-iam-permission-rejected: role granted only sts:GetCallerIdentity calls Sample. Expected error code AccessDeniedException, HTTP 403. Actual: ValidationException (HTTP 400). Assertion: `AssertionError: expected AccessDeniedException, got ValidationException` (check line 46). The fully-permissioned control call in the same check also returns ValidationException, i.e. authorized and unauthorized callers get identical answers.
- fine-grained-action-permission-rejected: role granted sagemaker:CompleteRollout + sagemaker:UpdateReward calls Sample. Expected AccessDeniedException/403; actual ValidationException/400 (check line 49).
- sample-vs-stream-action-independence-rejected: role granted only sagemaker:Sample calls SampleWithResponseStream. Expected AccessDeniedException/403; actual ValidationException/400 (check line 50).
- cross-account-resource-condition-rejected: role granted sagemaker:Sample with Condition StringEquals aws:ResourceAccount=${aws:PrincipalAccount}, called with a JobArn in account 999999999999. Expected AccessDeniedException/403; actual ValidationException/400 (check line 57).

Model evidence (services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/smithy-model/com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice.json): sdkId SagemakerJobRuntime, restJson1, sigv4; common_errors = [AccessDeniedException, InternalServiceError, ResourceNotFoundException, ServiceQuotaExceededException, ThrottlingException, ValidationException]; AccessDeniedException {errorKind client, httpError 403, required member Message}; ValidationException httpError 400; ResourceNotFoundException httpError 404. Sample/SampleWithResponseStream/CompleteRollout each list AccessDeniedException among their errors.

Message strings above are the standard IAM denial forms composed from the request (principal session ARN, IAM action, JobArn); the recorded texts are examples of that composition, while the 403/AccessDeniedException pair is the hard requirement.

## Steps to Reproduce
1. Against the emulator, create an IAM role with a trust policy allowing the account root to sts:AssumeRole, and attach an inline policy that does NOT grant the sagemakerjobruntime action under test (e.g. only {"Action":"sts:GetCallerIdentity","Resource":"*"}).
2. sts:AssumeRole into it and build a sagemakerjobruntime client from the returned temporary credentials.
3. Call Sample with a well-formed JobArn in the caller's own account (arn:aws:sagemaker:<region>:<account>:job/AgentRFT/<name>), any TrajectoryId, and a small JSON Body.
4. Observe ValidationException / HTTP 400 ("Could not find job with jobArn ..."); real AWS answers AccessDeniedException / HTTP 403.
5. Repeat with (a) a policy granting only sagemaker:CompleteRollout+UpdateReward and call Sample; (b) a policy granting only sagemaker:Sample and call SampleWithResponseStream; (c) a policy granting sagemaker:Sample with Condition StringEquals aws:ResourceAccount=${aws:PrincipalAccount} and call Sample with a JobArn in a different 12-digit account. All three should be AccessDeniedException/403 and all three return ValidationException/400.
6. In each check, the same role's GRANTED action must still clear authorization and reach the resource layer (CompleteRollout -> ResourceNotFoundException/404; Sample -> ValidationException/400), which is how the fix is distinguished from blanket denial.
