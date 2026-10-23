Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amp.resource-policy-stale-revision-conflict-rejected amp.anomaly-detector-create-delete-lifecycle
Issue-Shape-Id: com.amazonaws.amp#AmazonPrometheusService
Issue-Key: amp.resource-policy-stale-revision-conflict-rejected|run#13|AssertionError|assert code == "ValidationException", f"expected ValidationException on stale revisionId, got {code}"
Issue-Key: amp.anomaly-detector-create-delete-lifecycle|run#21|AssertionError|assert tagged == ("NotFoundException", "Resource was not found"), tagged
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# AMP: 2 error-code divergences on rejection paths (PutResourcePolicy stale revisionId answers ConflictException instead of ValidationException; TagResource on a deleted anomaly-detector ARN answers ResourceNotFoundException instead of NotFoundException)

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy (also TagResource)

## Description
Two independent root causes found in the amp (Amazon Prometheus Service) emulator by the behavioral suite. amp IS served by this emulator (its bootstrap issue is resolved and 9 of 22 amp checks pass), so these are fidelity defects on the rejection paths, not bootstrap gaps. TWO causes must be closed before submitting.

Cause 1 of 2: PutResourcePolicy with a STALE revisionId (optimistic-lock mismatch) is refused with the wrong error class — ConflictException (409) instead of ValidationException (400).
  AWS: Smithy (com.amazonaws.amp#PutResourcePolicy): input member `revisionId` is documented as "The revision ID of the policy to update. Use this parameter to ensure that you are updating the correct version of the policy. If you don't specify a revision ID, the policy is updated regardless of its current revision." The operation models BOTH `ValidationException` (errorKind client, @httpError 400) and `ConflictException` (errorKind client, @httpError 409), so the two are distinct modelled outcomes and only one is correct here. Recorded behavior: a PutResourcePolicy that supplies a revisionId which is no longer the current one is rejected with error code ValidationException (HTTP 400); recorded example message "Can't conditionally update a workspace resource policy with a wrong revisionId." (message quoted as an example only). The current policy document and revisionId are left untouched by the rejection, and a subsequent PutResourcePolicy carrying the CURRENT revisionId succeeds and advances the revisionId.
  Emulator: the same stale-revisionId PutResourcePolicy is refused with error code ConflictException (the modelled 409 shape) instead of ValidationException (400). Replay: `AssertionError: expected ValidationException on stale revisionId, got ConflictException`.
  Why: a client doing conditional/compare-and-swap updates of a workspace resource policy branches on the error code and its status class. A 400 ValidationException is a non-retryable "your revisionId is wrong — re-read and retry with the fresh one" signal; a 409 ConflictException is widely treated as a transient resource-state conflict and is retried as-is by SDK/retry wrappers and by hand-written `except ConflictException: sleep/retry` logic. So against this emulator a stale-write loop either spins retrying an error that can never succeed, or never reaches the ValidationException branch that re-reads the revision — and conversely code written against the emulator will mis-handle real AWS.
  Fix: the optimistic-concurrency check in the PutResourcePolicy handler for the amp service (services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/put_resource_policy.rs and the error shape it serializes) must classify a revisionId mismatch as ValidationException/400, keeping the existing no-effect semantics (policy document and revisionId unchanged) and keeping the current-revisionId path a success.
  Traces: amp.resource-policy-stale-revision-conflict-rejected

Cause 2 of 2: TagResource against the ARN of a resource that no longer exists (a deleted anomaly detector) is refused with error code ResourceNotFoundException instead of NotFoundException.
  AWS: recorded behavior only — the on-disk Smithy model lists `ResourceNotFoundException` (@httpError 404) on TagResource, but the recorded AWS response for an AMP resource ARN that does not exist uses error code NotFoundException with status 404 (recorded example message "Resource was not found"). The recorded code is the requirement; the message is an example.
  Emulator: refuses with error code ResourceNotFoundException (404). Replay: `AssertionError: ('ResourceNotFoundException', 'Resource not found.')` where the check expects `("NotFoundException", "Resource was not found")`. Note the rest of this lifecycle check passes: CreateAnomalyDetector reaches ACTIVE with the submitted configuration intact, DeleteAnomalyDetector removes it, and DescribeAnomalyDetector afterwards correctly answers ResourceNotFoundException — only the tagging-a-dead-ARN rejection uses the wrong code.
  Why: clients branch on the error code, not the message, so SDK-generated exception classes and `except NotFoundException` / create-if-missing handlers never match; tagging a deleted detector or workspace surfaces as an unexpected error instead of the modelled not-found path. It also makes the "gone means unusable" contract unverifiable for clients.
  Fix: the not-found rejection shared by the amp tagging handlers (services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/tag_resource.rs, and the matching UntagResource/ListTagsForResource paths) and the error shape they serialize must use code NotFoundException with status 404 for any ARN that resolves to no live resource — workspaces AND anomaly detectors. Related: open issue 20261022T110707Z-amp-3-fidelity-divergences-prometheusendpoint-announces-the-real.md describes this same wrong code for TagResource on a nonexistent WORKSPACE ARN (its Cause 2); that issue does not cover the anomaly-detector ARN path exercised here, so fixing it must cover every ARN type the tagging handlers resolve.
  Traces: amp.anomaly-detector-create-delete-lifecycle

## Evidence
amp.resource-policy-stale-revision-conflict-rejected [mismatch]:
  File "check_amp_resource_policy_stale_revision_conflict_rejected.py", line 63, in run
  assert code == "ValidationException", f"expected ValidationException on stale revisionId, got {code}"
  AssertionError: expected ValidationException on stale revisionId, got ConflictException
  Expected: PutResourcePolicy(workspaceId=W, policyDocument=<new doc>, revisionId=<stale>) -> HTTP 400, Error.Code=ValidationException (recorded example message "Can't conditionally update a workspace resource policy with a wrong revisionId."), policy left at the current revisionId/document; then the same call with the CURRENT revisionId succeeds and advances revisionId.
  Actual: refusal carries Error.Code=ConflictException (the modelled 409 shape).

amp.anomaly-detector-create-delete-lifecycle [mismatch]:
  File "check_amp_anomaly_detector_create_delete_lifecycle.py", line 72, in run
  assert tagged == ("NotFoundException", "Resource was not found"), tagged
  AssertionError: ('ResourceNotFoundException', 'Resource not found.')
  Expected: after DeleteAnomalyDetector, TagResource(resourceArn=<deleted detector arn>) -> 404 with Error.Code=NotFoundException (recorded example message "Resource was not found").
  Actual: 404 with Error.Code=ResourceNotFoundException.
  (Earlier beats of the same check passed: ad- id prefix, CREATING->ACTIVE, configuration round-trip, and DescribeAnomalyDetector post-delete -> ResourceNotFoundException "Anomaly detector not found.")

## Steps to Reproduce
Cause 1: amp.create_workspace(alias=...); poll describe_workspace until status ACTIVE. amp.put_resource_policy(workspaceId=W, policyDocument=<doc with Action aps:GetSeries>) -> keep its revisionId as STALE; poll describe_resource_policy until policyStatus ACTIVE; amp.put_resource_policy(workspaceId=W, policyDocument=<doc with Action aps:QueryMetrics>, revisionId=STALE) -> keep its revisionId as CURRENT; poll ACTIVE. Now amp.put_resource_policy(workspaceId=W, policyDocument=<doc with Action aps:RemoteWrite>, revisionId=STALE) — observe Error.Code ConflictException (409) instead of ValidationException (400). Control: the same call with revisionId=CURRENT must succeed and advance revisionId.

Cause 2: amp.create_workspace(...) and wait for ACTIVE; amp.create_anomaly_detector(workspaceId=W, alias=..., configuration={'randomCutForest': {'query': 'sum(up)', 'shingleSize': 8, 'sampleSize': 256}}, evaluationIntervalInSeconds=60) and keep the returned arn; wait for ACTIVE; amp.delete_anomaly_detector(workspaceId=W, anomalyDetectorId=...); poll describe_anomaly_detector until it 404s; then amp.tag_resource(resourceArn=<the detector arn>, tags={'k':'v'}) — observe Error.Code ResourceNotFoundException instead of NotFoundException.
