Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-harvest-job-workflow
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-harvest-job-workflow
Issue-Shape-Id: com.amazonaws.mediapackage#MediaPackage
Issue-Key: mediapackage|CreateHarvestJob|4xx->4xx|$.errors.originEndpointId
Issue-Key: mediapackage|CreateHarvestJob|4xx->4xx|$.message
Issue-Commit: 809356cd653f8299306e3123c06a94f56555f3b6

# CreateHarvestJob refuses the destination role before the origin endpoint's startover precondition, so the wrong member is reported

> _Note: this issue was identified against the code base at commit `809356cd653f8299306e3123c06a94f56555f3b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHarvestJob

## Description
Cause 1 of 1: CreateHarvestJob decides the destination role's trust policy BEFORE it decides the stored origin endpoint's startover precondition, so a submission that fails both is refused naming s3Destination.roleArn when AWS refuses it naming originEndpointId.

AWS: CreateHarvestJob models exactly one 4xx for an invalid submission, UnprocessableEntityException (HTTP 422, "The parameters sent in the request are not valid"), alongside ForbiddenException/NotFoundException/TooManyRequestsException/InternalServerErrorException/ServiceUnavailableException; its input is Id, OriginEndpointId, StartTime, EndTime and the required S3Destination{BucketName,ManifestKey,RoleArn} (Smithy: CreateHarvestJob, error UnprocessableEntityException httpError 422; S3Destination$RoleArn @jsonName roleArn). The model states no precedence between the destination role and the harvested endpoint, so the ordering here is recorded behavior only: for a submission whose origin endpoint retains no startover window, the recording shows AWS answering 422 with body member `errors.originEndpointId` = ["must have startover enabled"] and `message` = "originEndpointId must have startover enabled" — i.e. the state precondition on the resource being harvested is decided first, and the destination role is not what the caller is told to fix. In that message the fixed text is "must have startover enabled" and the only variable part is the member path the failure is keyed by ("originEndpointId", from the request's member name); the body carries the same reason twice, once under errors.<member> as a list of reasons and once as the flat message "<member> <reason>".

Emulator: the same step answers 422 but with `message` = "s3Destination.roleArn names a role mediapackage.amazonaws.com cannot assume: check the trust policy grants it sts:AssumeRole" and NO `errors.originEndpointId` entry at all (the replay reports `$.errors.originEndpointId: missing (expected ['must have startover enabled'], got None)` and `$.message: value_mismatch`). The status is right; the member keyed, the reason list and the message are not.

Why: a client that submits a harvest against an endpoint with startover playback disabled is told its destination role's trust policy is wrong. It then edits the role's trust policy (or re-creates the role), re-submits, and gets refused again, because the thing it actually has to fix — StartoverWindowSeconds on the origin endpoint — was never reported. Clients and tests that branch on the errors map key, or that assert the 422 names originEndpointId, take the wrong branch; an SDK error-message assertion fails outright.

Fix: the handler's pre-flight order in the CreateHarvestJob handler. The startover precondition read off the stored endpoint must be decided before the destination-role trust evaluation, so that a submission failing both is refused on originEndpointId, exactly as the recording shows; the role refusal remains the answer only when the endpoint precondition holds. Hypothesis (to confirm by re-replaying the step, not asserted): the other way this body can arise is the shared trust-policy evaluator judging a role that DOES grant mediapackage.amazonaws.com sts:AssumeRole as withholding the grant, which would be a false refusal rather than an ordering fault; whoever fixes this should check which of the two the fixture's role exercises before changing the evaluator.

Traces: mediapackage-harvest-job-workflow#task#3 (CreateHarvestJob, verdict mismatch, expected 422 == actual 422, diverging keys $.errors.originEndpointId and $.message).

Related open issues, neither of which covers this: 20261024T010718Z-mediapackage-createharvestjob-validation.md is about the destination BUCKET and a RoleArn naming no role at all never being validated (a missing refusal), while this is a refusal that fires in the wrong order and names the wrong member; 20261024T071559Z-mediapackage-createharvestjob-event-propagation.md is about an accepted job's terminal status.

## Evidence
Replay of fixture `mediapackage-harvest-job-workflow`, step `task#3` (service mediapackage, operation CreateHarvestJob, verdict mismatch, divergence_key `service=mediapackage|CreateHarvestJob|status=422->422|keys=$.errors.originEndpointId,$.message`):

Expected (recorded AWS), HTTP 422:
  $.errors.originEndpointId = ["must have startover enabled"]
  $.message = "originEndpointId must have startover enabled"

Actual (emulator), HTTP 422:
  $.errors.originEndpointId = absent
  $.message = "s3Destination.roleArn names a role mediapackage.amazonaws.com cannot assume: check the trust policy grants it sts:AssumeRole"

Code: services/service-com-amazonaws-mediapackage-mediapackage/src/ops/create_harvest_job.rs — the destination-role checks run at lines ~249-265 (`check_arn_member(HARVEST_ROLE_MEMBER, ...)` then `state::harvest_role_refusal(...)`, before the service store is even locked), while the endpoint precondition runs only at lines ~292-294 (`if !endpoint.startover_enabled() { return Err(state::startover_required("originEndpointId")) }`). The two refusal builders are services/service-com-amazonaws-mediapackage-mediapackage/src/state.rs:2306 (`startover_required` -> `unprocessable_state(member, "must have startover enabled")`) and :2348-2369 (`assume_role_refusal`/`harvest_role_refusal`, whose message text matches the actual body verbatim), so the body observed is unambiguously the role branch winning the race. The handler's own module docs (lines 26-40) list the endpoint check second of three, which is the order the wire contradicts.

## Steps to Reproduce
1. Replay fixture `mediapackage-harvest-job-workflow` against the emulator and stop at step `task#3`.
2. Equivalent by hand (region us-west-2, boto3 pointed at the emulator):
   a. iam.create_role(RoleName="harvest-dest", AssumeRolePolicyDocument=<a trust policy that does NOT grant mediapackage.amazonaws.com sts:AssumeRole, e.g. Principal.Service = "lambda.amazonaws.com">)
   b. mp.create_channel(Id="c1")
   c. mp.create_origin_endpoint(Id="oe1", ChannelId="c1", HlsPackage={})  # no StartoverWindowSeconds, so startover is disabled
   d. mp.create_harvest_job(Id="hj1", OriginEndpointId="oe1", StartTime=<past>, EndTime=<past+1h>, S3Destination={"BucketName": "b", "ManifestKey": "out/index.m3u8", "RoleArn": "<the ARN from step a>"})
3. Observe HTTP 422 whose message is the s3Destination.roleArn trust-policy refusal and whose errors map has no originEndpointId entry.
4. AWS answers HTTP 422 with errors.originEndpointId = ["must have startover enabled"] and message "originEndpointId must have startover enabled".
5. Re-run step 2d with StartoverWindowSeconds=86400 on the endpoint: the roleArn refusal is then the correct answer, which is the ordering this issue asks for.
