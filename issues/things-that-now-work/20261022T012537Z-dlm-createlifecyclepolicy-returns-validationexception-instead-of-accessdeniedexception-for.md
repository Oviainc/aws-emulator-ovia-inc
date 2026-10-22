Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dlm.create-policy-invalid-execution-role-arn-rejected
Issue-Shape-Id: com.amazonaws.dlm#dlm_20180112
Issue-Key: dlm.create-policy-invalid-execution-role-arn-rejected|run#7|AssertionError|assert code == "AccessDeniedException", (
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# dlm CreateLifecyclePolicy returns ValidationException instead of AccessDeniedException for a non-role/cross-account ExecutionRoleArn

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLifecyclePolicy

## Description
One root cause (Cause 1 of 1).

Cause 1 of 1: CreateLifecyclePolicy rejects an ExecutionRoleArn that is not an IAM *role* ARN (here an IAM user ARN in a foreign account, `arn:aws:iam::111122223333:user/not-a-role`) with the wrong error class.

AWS: recorded behavior only for the error class — the DLM Smithy model (services/service-com-amazonaws-dlm-dlm-20180112/smithy-model/com-amazonaws-dlm-dlm-20180112.json) declares only InternalServerException (500), InvalidRequestException (400) and LimitExceededException (429) for CreateLifecyclePolicy, and constrains CreateLifecyclePolicyRequest$ExecutionRoleArn with @length 0..2048 and @pattern `^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/`. The recorded AWS response for an ExecutionRoleArn that does not denote a role the caller may pass is the error code **AccessDeniedException** (DLM attempts the iam:PassRole and is denied), with the recorded message `Cross-account pass role is not allowed.` as the example text the check asserts. The rejected create must leave no policy behind, and a well-formed role ARN must still succeed (positive control in the same check). Hypothesis: AWS performs the PassRole authorization check ahead of (or instead of) the member pattern check, so the authorization failure is what surfaces.

Emulator: the request is refused by the framework's generic member-constraint validation against the ExecutionRoleArn @pattern, producing error code `ValidationException`; the handler itself (services/service-com-amazonaws-dlm-dlm-20180112/src/ops/create_lifecycle_policy.rs, handle at lines 178-283) contains no execution-role validation at all, so there is no path that can emit AccessDeniedException.

Why: a client that keys its error handling on the error code sees a client-side input-shape error rather than an authorization denial. Retry/alerting logic that distinguishes "the caller lacks iam:PassRole for this role" (operator action: fix the role or the grant) from "the request body is malformed" (operator action: fix the ARN string) takes the wrong branch, and callers that catch AccessDeniedException let an unexpected ValidationException escape.

Fix: the CreateLifecyclePolicy handler's execution-role validation — the ExecutionRoleArn check must run as an authorization/pass-role step in the handler (or ahead of generic pattern validation for that member) and answer AccessDeniedException for an ARN that is not a role ARN the caller may pass, instead of letting the member @pattern produce ValidationException. The rejected create must still not insert a policy record.

## Evidence
Check dlm.create-policy-invalid-execution-role-arn-rejected (verdict: mismatch).

Request: CreateLifecyclePolicy with ExecutionRoleArn="arn:aws:iam::111122223333:user/not-a-role", Description=<unique>, State="ENABLED", PolicyDetails=<EBS_SNAPSHOT_MANAGEMENT, ResourceTypes=[VOLUME], one 24h schedule>.

Expected (recorded AWS): error code AccessDeniedException; message example "Cross-account pass role is not allowed."; no policy created.
Actual (emulator): error code 'ValidationException'.

Assertion from the replay:
  File "check_dlm_create_policy_invalid_execution_role_arn_rejected.py", line 42, in run
    assert code == "AccessDeniedException", (
  AssertionError: expected AccessDeniedException when ExecutionRoleArn is not a role ARN the caller may pass, got 'ValidationException'

## Steps to Reproduce
1. Boot the emulator and point a botocore dlm client at it.
2. Call create_lifecycle_policy(ExecutionRoleArn="arn:aws:iam::111122223333:user/not-a-role", Description="sim2real-dlm-desc-x", State="ENABLED", PolicyDetails={"PolicyType":"EBS_SNAPSHOT_MANAGEMENT","ResourceTypes":["VOLUME"],"TargetTags":[{"Key":"sim2real-dlm-target","Value":"bad-role-arn"}],"Schedules":[{"Name":"sim2real-sched","CreateRule":{"Interval":24,"IntervalUnit":"HOURS","Times":["03:00"]},"RetainRule":{"Count":1}}]}).
3. Inspect ClientError.response["Error"]["Code"]: the emulator answers 'ValidationException'; AWS answers 'AccessDeniedException'.
4. Confirm with get_lifecycle_policies(ResourceTypes=["VOLUME"], TargetTags=["sim2real-dlm-target=bad-role-arn"]) that no policy with that description was created (this part already holds).
