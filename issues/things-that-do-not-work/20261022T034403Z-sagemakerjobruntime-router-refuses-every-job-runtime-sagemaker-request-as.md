Issue-Source: trace-sweep-divergence
Issue-Trace: sagemakerjobruntime-complete-rollout-nonexistent-job-not-found
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemakerjobruntime-complete-rollout-nonexistent-job-not-found sagemakerjobruntime-resource-not-found-nonexistent-job sagemakerjobruntime-validation-errors sagemakerjobruntime-update-reward-resource-not-found
Issue-Shape-Id: com.amazonaws.sagemakerjobruntime#AgenticRFTRuntimeService
Issue-Key: sagemakerjobruntime|CompleteRollout|4xx->5xx|$.Message
Issue-Key: sagemakerjobruntime|CompleteRollout|4xx->5xx|$headers.content-type
Issue-Key: sagemakerjobruntime|CompleteRollout|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: sagemakerjobruntime|CompleteRollout|4xx->5xx|status
Issue-Key: sagemakerjobruntime|Sample|4xx->5xx|$.Message
Issue-Key: sagemakerjobruntime|Sample|4xx->5xx|$headers.content-type
Issue-Key: sagemakerjobruntime|Sample|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: sagemakerjobruntime|Sample|4xx->5xx|status
Issue-Key: sagemakerjobruntime|UpdateReward|4xx->5xx|$.Message
Issue-Key: sagemakerjobruntime|UpdateReward|4xx->5xx|$headers.content-type
Issue-Key: sagemakerjobruntime|UpdateReward|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: sagemakerjobruntime|UpdateReward|4xx->5xx|status
Issue-Key: sagemakerjobruntime|SampleWithResponseStream|4xx->5xx|$.Message
Issue-Key: sagemakerjobruntime|SampleWithResponseStream|4xx->5xx|$headers.content-type
Issue-Key: sagemakerjobruntime|SampleWithResponseStream|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: sagemakerjobruntime|SampleWithResponseStream|4xx->5xx|status
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# Router refuses every job-runtime.sagemaker request as AmbiguousService, so no SagemakerJobRuntime error path is reachable

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CompleteRollout

## Description
This bundle has 6 causes, covering all 4 bound operations of SagemakerJobRuntime (AgenticRFTRuntimeService, apiVersion 2026-02-01). All six share a single root cause in the shared request router, but each also names a distinct modeled rejection that must answer correctly once routing lands. ALL SIX must be closed before submitting.

SHARED ROOT CAUSE: the router resolves an incoming request to an emulated service by its sigv4 signing name. Smithy (services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/smithy-model/com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice.json, query=service_info) gives this service sigv4Name "sagemaker" and arnNamespace "sagemaker", while its endpointPrefix is the distinctive "job-runtime.sagemaker". Many emulated SageMaker-family services share the signing name "sagemaker" (sagemaker, sagemaker-runtime, sagemaker-runtime-http2, sagemaker-a2i-runtime, sagemaker-featurestore-runtime, sagemaker-metrics, sagemaker-edge, sagemaker-geospatial, sagemaker-training-session-runtime, sagemaker-job-runtime), so the router finds multiple claimants, refuses to pick one, and answers HTTP 500 with x-amzn-errortype AmbiguousService. No AWS model defines an AmbiguousService error, and AWS never answers 500 to any of the recorded client-fault requests. Two further defects ride along: the refusal is serialized with content-type application/x-amz-json-1.1 (awsJson1_1) although this service's protocol is restJson1, whose error documents are application/json with a `Message` member; and because the request is never routed, none of the operations' modeled input validation or state lookups run at all, so the response body carries no Message key whatsoever.

AWS (common to all six): protocol restJson1, so every error document is content-type application/json with an x-amzn-errortype header naming the modeled error and a required `Message` member (Smithy: ValidationException.Message and ResourceNotFoundException.Message target FailureReason, @length 0..1024, @required). Service-common errors are AccessDeniedException(403), InternalServiceError(500), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400); CompleteRollout and UpdateReward add ConflictException(409). The recordings show the front door emitting the x-amzn-errortype in Coral-suffixed form, e.g. 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.agenticrftruntimeservice/'; SDKs read the code before the colon, so the requirement is the error code ValidationException plus HTTP 400.

Emulator (common to all six): HTTP 500, content-type application/x-amz-json-1.1, x-amzn-errortype AmbiguousService, and no `Message` member in the body.

Why (common to all six): the caller gets a 500 server-fault where AWS returns a 4xx client-fault, so SDK retry policies classify it as retryable and retry a request that can never succeed, and the modeled ValidationException / ResourceNotFoundException branch never fires. The awsJson1_1 content-type and the unmodeled errortype also break restJson1 error-shape matching, so even a client that inspects the body cannot recover a typed error. Because the refusal happens in routing, the whole service is unreachable — not merely mis-validating — so every agentic-RFT rollout workload (Sample, SampleWithResponseStream, CompleteRollout, UpdateReward) fails outright.

Fix (common to all six): the shared request router / service-registry claim resolution — disambiguate claimants by endpointPrefix and the request host/URI (the restJson1 path bindings /sample, /sample-with-response-stream, /complete-rollout, /update-reward are themselves distinctive) rather than by sigv4 signing name alone; and the refusal serializer must emit the target service's protocol (restJson1: application/json plus x-amzn-errortype) instead of an awsJson1_1 envelope. Once routing lands, each operation's input validation and its trajectory/job state lookup must produce the modeled rejection described per cause below.

Cause 1 of 6: CompleteRollout against a trajectory that does not exist. AWS: 404 ResourceNotFoundException (Smithy: CompleteRollout errors include ResourceNotFoundException, httpError 404); the recorded Message is built from the request and the caller account — 'Trajectory <trajectoryId> not found for account <accountId>' (recorded example: 'Trajectory sim2real-wire-sjr-traj-778a2a85 not found for account 000000000000'), so the rule, not the literal text, is the requirement. Emulator: 500 AmbiguousService as above, Message absent. Fix: CompleteRollout's handler/state lookup. Trace: sagemakerjobruntime-complete-rollout-nonexistent-job-not-found step task#1.

Cause 2 of 6: Sample with a JobArn that names no existing job. AWS: recorded behavior is 400 with error code ValidationException (not 404), Message 'Could not find job with jobArn <arn>' (recorded example arn: arn:aws:sagemaker:us-west-2:000000000000:job/AgentRFT/sim2real-probe-sagemakerjobruntime-ghost-cdf5a33f); the Message is built from the request's JobArn. Smithy models JobArn as @required, @length 1..256, @pattern ^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/[a-zA-Z0-9_\-]+/[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$ bound to header X-Amzn-SageMaker-Job-Arn, and Sample's modeled errors include both ValidationException(400) and ResourceNotFoundException(404) — the recording shows the service choosing the 400 ValidationException for an unknown-but-well-formed job ARN. Emulator: 500 AmbiguousService as above. Fix: Sample's handler/job-resolution path. Trace: sagemakerjobruntime-resource-not-found-nonexistent-job step task#1 (this cluster also covers sagemakerjobruntime-validation-errors step task#1).

Cause 3 of 6: UpdateReward against a trajectory that does not exist. AWS: 404 ResourceNotFoundException (Smithy: UpdateReward errors include ResourceNotFoundException, httpError 404), Message built from the request trajectoryId and the caller account — 'Trajectory <trajectoryId> not found for account <accountId>' (recorded example: 'Trajectory sim2real-wire-sjr-traj-8bca0ce6 not found for account 000000000000'). Emulator: 500 AmbiguousService as above. Fix: UpdateReward's handler/state lookup. Trace: sagemakerjobruntime-update-reward-resource-not-found step task#1.

Cause 4 of 6: SampleWithResponseStream with a TrajectoryId longer than the modeled maximum. AWS: 400 ValidationException. Smithy: SampleWithResponseStream.TrajectoryId targets com.amazonaws.sagemakerjobruntime#TrajectoryId, @length min 1 max 256, @required, bound to header X-Amzn-SageMaker-Trajectory-Id. The Message is built from the request and the violated constraint — '1 validation error detected: Value '<value>' at 'trajectoryId' failed to satisfy constraint: Member must have length less than or equal to 256' (recorded with a 280-character 't' string); that rule is the requirement. Emulator: 500 AmbiguousService as above, no validation performed. Fix: SampleWithResponseStream's input validation. Trace: sagemakerjobruntime-validation-errors step task#2.

Cause 5 of 6: CompleteRollout with a Status outside the modeled enum. AWS: 400 ValidationException. Smithy: CompleteRollout.Status targets CompletionStatus, an enum whose values are exactly [ready, failed]. The Message is built from the request and the enum set — '1 validation error detected: Value '<value>' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ready, failed]' (recorded value 'bogus-status'). Emulator: 500 AmbiguousService as above, no enum check performed. Fix: CompleteRollout's input validation. Trace: sagemakerjobruntime-validation-errors step task#3.

Cause 6 of 6: UpdateReward with an empty Rewards list. AWS: 400 ValidationException with Message 'Rewards list cannot be empty'. Smithy models Rewards as @required targeting DoubleList, which declares no @length minimum, so the empty-list refusal is recorded behavior only rather than a modeled constraint — the requirement is the 400 status and the ValidationException code; the quoted sentence is a fixed service message observed in the recording. Emulator: 500 AmbiguousService as above, the empty list accepted into routing and never validated. Fix: UpdateReward's input validation. Trace: sagemakerjobruntime-validation-errors step task#4.

Related open issues (neither covers these steps): '20261021T234924Z-featurestore-runtime-sagemaker-router-refuses-every-featurestore-runtime-sagemaker-request-as' reports the same router defect for the featurestore-runtime.sagemaker claimant, and '20261021T234042Z-osis-router-refuses-every-sigv4-iam-request-as-ambiguousservice' reports it for the iam/iam-toolbox collision. Neither carries a replay key for any sagemakerjobruntime fixture, and neither covers this service's four operations or the six modeled rejections above, so fixing either alone will not close these six causes. A fix that generalises claim resolution should close all of them, but the per-operation validation and state lookups in causes 1-6 are specific to this service.

## Evidence
Replay of simparity-wire-aws-v1 against the booted emulator: steps_total=7, steps_passed=0, steps_mismatched=7, score_pct=0.0, emulator_up=true ("/__admin/health answered 200 and the :37051 data listener accepts"), bootstrap_signal=false, state_reset_failures=0, not_implemented=0, steps_blocked=0. 6 divergence clusters, all service=sagemakerjobruntime, none with a stub_path and none owned_by an existing issue.

Every cluster reports the same three shared diffs:
  $headers.content-type: expected 'application/json', got 'application/x-amz-json-1.1'
  $headers.x-amzn-errortype: expected the modeled error, got 'AmbiguousService'
  $.Message: missing (expected the modeled message, got None)

Per cluster, expected -> actual status and expected Message:
1. CompleteRollout, fixture sagemakerjobruntime-complete-rollout-nonexistent-job-not-found#task#1: 404 -> 500; $.Message expected 'Trajectory sim2real-wire-sjr-traj-778a2a85 not found for account 000000000000'; expected errortype shaped-not-asserted (modeled ResourceNotFoundException, httpError 404).
2. Sample, fixture sagemakerjobruntime-resource-not-found-nonexistent-job#task#1 (cluster also covers sagemakerjobruntime-validation-errors#task#1): 400 -> 500; $.Message expected 'Could not find job with jobArn arn:aws:sagemaker:us-west-2:000000000000:job/AgentRFT/sim2real-probe-sagemakerjobruntime-ghost-cdf5a33f'; errortype expected 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.agenticrftruntimeservice/'.
3. UpdateReward, fixture sagemakerjobruntime-update-reward-resource-not-found#task#1: 404 -> 500; $.Message expected 'Trajectory sim2real-wire-sjr-traj-8bca0ce6 not found for account 000000000000'; expected errortype shaped-not-asserted (modeled ResourceNotFoundException, httpError 404).
4. SampleWithResponseStream, fixture sagemakerjobruntime-validation-errors#task#2: 400 -> 500; $.Message expected "1 validation error detected: Value 'ttt...ttt' (280 chars) at 'trajectoryId' failed to satisfy constraint: Member must have length less than or equal to 256"; errortype expected 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.agenticrftruntimeservice/'.
5. CompleteRollout, fixture sagemakerjobruntime-validation-errors#task#3: 400 -> 500; $.Message expected "1 validation error detected: Value 'bogus-status' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ready, failed]"; errortype expected 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.agenticrftruntimeservice/'.
6. UpdateReward, fixture sagemakerjobruntime-validation-errors#task#4: 400 -> 500; $.Message expected 'Rewards list cannot be empty'; errortype expected 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.agenticrftruntimeservice/'.

Smithy evidence (services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/smithy-model/com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice.json):
- service_info: service AgenticRFTRuntimeService, version 2026-02-01, sdkId 'SagemakerJobRuntime', protocols [restJson1], authSchemes [sigv4], sigv4Name 'sagemaker', awsService.arnNamespace 'sagemaker', awsService.endpointPrefix 'job-runtime.sagemaker'. common_errors: AccessDeniedException, InternalServiceError, ResourceNotFoundException, ServiceQuotaExceededException, ThrottlingException, ValidationException.
- digest: 4 bound operations — Sample (POST /sample), SampleWithResponseStream (POST /sample-with-response-stream), CompleteRollout (POST /complete-rollout, +ConflictException), UpdateReward (POST /update-reward, +ConflictException).
- operation CompleteRollout: errors include ResourceNotFoundException httpError 404 and ValidationException httpError 400; input JobArn (header X-Amzn-SageMaker-Job-Arn, required, @length 1..256, @pattern ^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/[a-zA-Z0-9_\-]+/[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$), TrajectoryId (required, @length 1..256), Status (CompletionStatus).
- operation UpdateReward: same error set; input Rewards target DoubleList, @required.
- operation Sample / SampleWithResponseStream: errors include ValidationException 400 and ResourceNotFoundException 404; TrajectoryId bound to header X-Amzn-SageMaker-Trajectory-Id, @required, @length 1..256.
- shape CompletionStatus: enum, values exactly ['ready', 'failed'].
- shape TrajectoryId: string, @length min 1 max 256.
- shape DoubleList: list of Double, no @length constraint (hence cause 6 is recorded behavior only).
- shapes ValidationException / ResourceNotFoundException: structures with a @required Message member (FailureReason, @length 0..1024), errorKind client, httpError 400 / 404.
- No AWS model defines an 'AmbiguousService' error, and restJson1 error documents are application/json, never application/x-amz-json-1.1.

## Steps to Reproduce
1. git clone the repo and boot the emulator from that checkout (/__admin/health answers 200 and the data listener accepts connections).
2. Point an SDK/HTTP client for SagemakerJobRuntime at the emulator's data listener: protocol restJson1, sigv4 signing name 'sagemaker', endpointPrefix 'job-runtime.sagemaker'.
3. Cause 1: POST /complete-rollout with header X-Amzn-SageMaker-Job-Arn set to a well-formed job ARN and a TrajectoryId that was never created. Observe HTTP 500, content-type application/x-amz-json-1.1, x-amzn-errortype AmbiguousService and no Message member, instead of HTTP 404 application/json ResourceNotFoundException with Message 'Trajectory <trajectoryId> not found for account <accountId>'.
4. Cause 2: POST /sample with X-Amzn-SageMaker-Job-Arn naming a job that does not exist. Observe the same 500 AmbiguousService refusal instead of HTTP 400 application/json ValidationException with Message 'Could not find job with jobArn <arn>'.
5. Cause 3: POST /update-reward with a TrajectoryId that was never created. Observe the same 500 refusal instead of HTTP 404 ResourceNotFoundException with Message 'Trajectory <trajectoryId> not found for account <accountId>'.
6. Cause 4: POST /sample-with-response-stream with header X-Amzn-SageMaker-Trajectory-Id set to a 280-character value (TrajectoryId @length max 256). Observe the same 500 refusal instead of HTTP 400 ValidationException naming 'trajectoryId' and the length-256 constraint.
7. Cause 5: POST /complete-rollout with Status='bogus-status'. Observe the same 500 refusal instead of HTTP 400 ValidationException naming 'status' and the enum value set [ready, failed].
8. Cause 6: POST /update-reward with Rewards=[] (empty list). Observe the same 500 refusal instead of HTTP 400 ValidationException with Message 'Rewards list cannot be empty'.
9. Confirm the refusal is routing-level, not per-operation: every one of the four bound operations refuses identically with x-amzn-errortype AmbiguousService, so no SagemakerJobRuntime operation is reachable at all.
10. Replay the wire corpus simparity-wire-aws-v1 and re-run all six fixture steps to confirm the fix: sagemakerjobruntime-complete-rollout-nonexistent-job-not-found#task#1, sagemakerjobruntime-resource-not-found-nonexistent-job#task#1, sagemakerjobruntime-update-reward-resource-not-found#task#1, sagemakerjobruntime-validation-errors#task#2, sagemakerjobruntime-validation-errors#task#3, sagemakerjobruntime-validation-errors#task#4 (plus sagemakerjobruntime-validation-errors#task#1, the second member of the Sample cluster).
