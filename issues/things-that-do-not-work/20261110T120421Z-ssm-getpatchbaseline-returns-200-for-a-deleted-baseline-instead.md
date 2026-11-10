Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-create-patch-baseline-and-inspect-patches
Issue-Step: teardown#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-create-patch-baseline-and-inspect-patches ssm-default-patch-baseline-get-register-restore ssm-default-patch-baseline-lifecycle ssm-describe-effective-patches-for-new-baseline ssm-patch-baseline-create-describe-effective-delete
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|GetPatchBaseline|4xx->2xx|$.Message
Issue-Key: ssm|GetPatchBaseline|4xx->2xx|$.__type
Issue-Key: ssm|GetPatchBaseline|4xx->2xx|status
Issue-Commit: 4ee873d489ba3f816ace3dde9276e9176cccca43

# GetPatchBaseline returns 200 for a deleted baseline instead of 400 DoesNotExistException

> _Note: this issue was identified against the code base at commit `4ee873d489ba3f816ace3dde9276e9176cccca43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPatchBaseline

## Description
Cause 1 of 1: GetPatchBaseline never checks that the requested baseline still exists, so a baseline that has already been removed by DeletePatchBaseline keeps answering 200 with its full body.

AWS: Smithy (services/service-com-amazonaws-ssm-amazonssm) models GetPatchBaseline with input BaselineId (required, @length 20..128, pattern ^[a-zA-Z0-9_\-:/]{20,128}$) and errors DoesNotExistException (client, httpError 400), InvalidResourceId and InternalServerError. DoesNotExistException is documented as "Error returned when the ID specified for a resource, such as a maintenance window or patch baseline, doesn't exist" and carries a single member Message. Once DeletePatchBaseline has succeeded for a baseline id, a subsequent GetPatchBaseline for that id is refused: HTTP 400, awsJson error code (__type) 'DoesNotExistException', with Message 'Patch Baseline to be retrieved does not exist.' That message is entirely fixed text for this error class on this operation -- there is no variable part interpolated from the request or from state (the baseline id is NOT echoed into it), so the whole string is the requirement.

Emulator: the request is answered HTTP 200 with a normal GetPatchBaseline result payload. No __type and no Message are emitted at all (replay reports `status: expected 400, got 200`, `$.__type: missing (expected 'DoesNotExistException', got None)`, `$.Message: missing (expected 'Patch Baseline to be retrieved does not exist.', got None)`). The deleted baseline is therefore still readable after its delete returned success.

Why: a client cannot observe deletion. Teardown and reconciliation logic that deletes a patch baseline and then polls GetPatchBaseline until it is refused never terminates, because the emulator keeps serving the object; cleanup verification, "delete is idempotent/observable" assertions, and any code that distinguishes a live baseline from a removed one by catching DoesNotExistException all take the wrong branch and treat the baseline as still present. Error-handling paths for a bad or stale baseline id are never exercised.

Fix: the GetPatchBaseline handler and the patch-baseline state it reads. The lookup must resolve the baseline id against live state and, when no live record is found (including ids whose record was removed by DeletePatchBaseline), raise the modeled DoesNotExistException so the awsJson error serializer emits HTTP 400 with __type 'DoesNotExistException' and Message 'Patch Baseline to be retrieved does not exist.' -- i.e. deletion must actually remove the record (or mark it unreadable) rather than leaving it retrievable. The same existence check belongs on any other read that resolves a baseline id.

Traces: the cluster's representative is ssm-create-patch-baseline-and-inspect-patches#teardown#5; the same single cause also surfaces at ssm-default-patch-baseline-get-register-restore#teardown#7, ssm-default-patch-baseline-lifecycle#task#9, ssm-describe-effective-patches-for-new-baseline#teardown#4 and ssm-patch-baseline-create-describe-effective-delete#teardown#5 (5 steps, one divergence cluster). No existing open issue covers this: the open ssm patch-baseline issues are unimplemented-operation stubs (CreatePatchBaseline, UpdatePatchBaseline, DescribePatchBaselines, DescribeAvailablePatches, DescribePatchProperties, DescribeEffectivePatchesForPatchBaseline, RegisterPatchBaselineForPatchGroup, DescribeInstancePatchStatesForPatchGroup) and 20261109T073558Z-cloudformation-ec2-ssm-a-cfn-deployed-ssm-patch.md, which is about patch compliance never being produced, not about GetPatchBaseline's post-delete existence check.

## Evidence
Fixture ssm-create-patch-baseline-and-inspect-patches, step teardown#5, operation GetPatchBaseline (service ssm).

Expected (recorded AWS response): HTTP 400, body {"__type": "DoesNotExistException", "Message": "Patch Baseline to be retrieved does not exist."}

Actual (emulator): HTTP 200 with a GetPatchBaseline result body; $.__type missing, $.Message missing.

Replay detail verbatim:
  status: expected 400, got 200
  $.__type: missing (expected 'DoesNotExistException', got None)
  $.Message: missing (expected 'Patch Baseline to be retrieved does not exist.', got None)

Divergence key: service=ssm|GetPatchBaseline|status=400->200|keys=$.Message,$.__type,status
Cluster size: 5 affected steps (listed under Traces). Verdict: mismatch. No stub_path, no owning issue.

Message anatomy: 'Patch Baseline to be retrieved does not exist.' is 100% fixed text for this error class; nothing in it is taken from the request or emulator state.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay fixture ssm-create-patch-baseline-and-inspect-patches: CreatePatchBaseline for a new baseline, inspect it, then DeletePatchBaseline for the returned BaselineId.
3. At teardown#5, call ssm GetPatchBaseline with that same (now deleted) BaselineId.
4. Observe HTTP 200 with a full baseline body. AWS instead answers HTTP 400 with __type 'DoesNotExistException' and Message 'Patch Baseline to be retrieved does not exist.'
5. The same sequence reproduces at ssm-default-patch-baseline-get-register-restore#teardown#7, ssm-default-patch-baseline-lifecycle#task#9, ssm-describe-effective-patches-for-new-baseline#teardown#4 and ssm-patch-baseline-create-describe-effective-delete#teardown#5.
