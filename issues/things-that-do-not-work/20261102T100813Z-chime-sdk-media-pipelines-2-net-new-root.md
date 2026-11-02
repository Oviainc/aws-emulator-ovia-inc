Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chime-sdk-media-pipelines.get-media-pipeline-not-found chime-sdk-media-pipelines.kinesis-video-stream-pool-duplicate-name-rejected
Issue-Shape-Id: com.amazonaws.chimesdkmediapipelines#ChimeSDKMediaPipelinesService
Issue-Key: chime-sdk-media-pipelines.get-media-pipeline-not-found|run#9|AssertionError|assert error["Message"] == "Unable to find the requested identifier", (
Issue-Key: chime-sdk-media-pipelines.kinesis-video-stream-pool-duplicate-name-rejected|run#9|AssertionError|assert error["Code"] == "BadRequestException", (
Issue-Commit: 699db40da250628f4cc12f3dc175d789bc27ad1e

# chime-sdk-media-pipelines: 2 net-new root causes — GetMediaPipeline's NotFoundException message is composed prose interpolating the requested id, and a duplicate PoolName is refused 409 ConflictException instead of 400 BadRequestException

> _Note: this issue was identified against the code base at commit `699db40da250628f4cc12f3dc175d789bc27ad1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMediaPipeline

## Description
Two distinct root causes in the chime-sdk-media-pipelines emulator, both grounded in failing behavioral checks of this one service. These are the only two failing checks of this service the replay report did NOT mark OWNED/ALREADY FILED; the other ten failures are marked OWNED by 20261029T171129Z-chime-sdk-media-pipelines-7-net-new-root.md and are skipped here.

Related but non-covering open issues (named for the fixer's benefit, they do not carry these checks' current replay keys): 20261029T171129Z-chime-sdk-media-pipelines-7-net-new-root.md describes the same two emulator defects in its Cause 1 and Cause 2, but the replay report reports both checks below as NOT owned — their Issue-Key frame offsets in that issue (run#8) no longer match the checks as replayed, so neither check is gated by it and a fix closed against it would leave both unrun. 20261029T175151Z-chime-sdk-media-pipelines-notfoundexception-message-is-composed-prose-that-interpolates-the.md covers the same wording defect on DeleteMediaInsightsPipelineConfiguration only, from the WIRE corpus, and carries no behavioral check id at all. Both causes below are still reproducible in the current checkout at the cited lines.

=== Cause 1 of 2: GetMediaPipeline's NotFoundException message is a per-resource sentence with the requested MediaPipelineId substituted into it, instead of this service's one fixed not-found text ===
AWS: Smithy: GetMediaPipeline, GET /sdk-media-pipelines/{MediaPipelineId}, success 200, models NotFoundException with httpError 404 (documented "One or more of the resources in the request does not exist in the system."); its MediaPipelineId member is an @required httpLabel GuidString, @length 36..36, @pattern ^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$. The model does not state the Message text, so for the wording: recorded behavior only — when MediaPipelineId names no existing media pipeline, the response is HTTP 404 with Code NotFoundException and Message exactly the fixed string "Unable to find the requested identifier". That message is ENTIRELY fixed text: no variable parts at all — no resource-kind noun, no id, no ARN — so nothing from the request or from emulator state is interpolated into it.
Emulator: the HTTP 404 and the Code NotFoundException are correct, but the handler composes its own sentence and substitutes the caller's id into it: Message is "No media pipeline with the ID <MediaPipelineId> exists." — fixed prose "No media pipeline with the ID " + the MediaPipelineId copied verbatim from the request path + " exists." (the replayed check passed a well-formed but nonexistent GUID and got it echoed back in full). Only the message text diverges.
Why: a client that branches on, matches, or logs this service's not-found wording behaves differently against the emulator than against AWS. AWS's text deliberately echoes NO caller-supplied identifier, while the emulator's echoes the whole id, so an emulator-based test of an error-redaction or log-scrubbing path sees identifier material AWS never emits, and error-message equality checks fail.
Fix: the service's not-found refusal wording — crate::state::not_found is already the single home for building this error, but the handler passes it a format!-composed, id-interpolated string; the fixed text belongs in that one helper so no handler words it or interpolates an identifier into it. The lookup logic and the 404/NotFoundException classification are already right.
Traces: chime-sdk-media-pipelines.get-media-pipeline-not-found (AssertionError: unexpected NotFoundException message: 'No media pipeline with the ID 00000000-0000-0000-0000-000000000000 exists.'). Source: services/service-com-amazonaws-chimesdkmediapipelines-chimesdkmediapipelinesservice/src/ops/get_media_pipeline.rs lines 191-197.

=== Cause 2 of 2: a duplicate PoolName is refused 409 ConflictException instead of 400 BadRequestException ===
AWS: Smithy: CreateMediaPipelineKinesisVideoStreamPool, POST /media-pipeline-kinesis-video-stream-pools, success 201, models BOTH BadRequestException (httpError 400) and ConflictException (httpError 409), so the model alone does not pick one; PoolName is @required, @length 1..128, @pattern ^[0-9a-zA-Z._-]+$. Recorded behavior: a create whose PoolName is already in use is refused HTTP 400 with Code BadRequestException and Message "Media Pipeline Kinesis Video Stream Pool already exists with <PoolName> name" — fixed text "Media Pipeline Kinesis Video Stream Pool already exists with " + the PoolName taken verbatim from the request + " name". The pre-existing pool is left untouched (it stays the only pool under that name, with its original DataRetentionInHours), and a create under a different name succeeds.
Emulator: answers Code ConflictException (HTTP 409) with Message "A Kinesis Video Stream pool with that name already exists." — a different error code, a different HTTP status, and a message that names no PoolName at all.
Why: retry middleware and client code classify 409 Conflict as a retryable/raceable condition and 400 as a permanent input error, so a client that fails fast against AWS retries against the emulator; and a client that parses the colliding PoolName out of the message to report which name collided gets nothing, since the emulator's message carries no name.
Fix: the CreateMediaPipelineKinesisVideoStreamPool handler's name-in-use refusal — the StreamPoolRefused::NameInUse arm must raise the modeled BadRequestException with AWS's wording (PoolName interpolated as above) rather than crate::state::conflict.
Traces: chime-sdk-media-pipelines.kinesis-video-stream-pool-duplicate-name-rejected (AssertionError: expected BadRequestException for duplicate PoolName, got ConflictException). Source: services/service-com-amazonaws-chimesdkmediapipelines-chimesdkmediapipelinesservice/src/ops/create_media_pipeline_kinesis_video_stream_pool.rs lines 247-256.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1): 10/22 passed (45.5%); chime-sdk-media-pipelines 10/22, 12 failed, 0 errored, 0 skipped, bootstrap_signal false (the emulator serves — 10 checks of this service pass). Ten of the twelve failures are marked "OWNED by 20261029T171129Z-chime-sdk-media-pipelines-7-net-new-root.md — already filed; skip it." and are excluded. The two unowned failures are clustered into the two causes above.

Cause 1 — GetMediaPipeline on a nonexistent MediaPipelineId:
  Required: HTTP 404, Code "NotFoundException", Message "Unable to find the requested identifier" — 100% fixed text, no variable parts, and the requested id must appear nowhere in the body.
  Observed: HTTP 404 and Code "NotFoundException" (both correct), Message "No media pipeline with the ID 00000000-0000-0000-0000-000000000000 exists."
  Fixed/variable split of the observed message: fixed prose "No media pipeline with the ID " and " exists."; variable part the MediaPipelineId taken from the request path. The GUID quoted here is the per-run value the check happened to pass and is NOT part of the requirement — the defect is that the template interpolates any identifier at all, and that the surrounding prose is not AWS's text.
  Check assertion: assert error["Message"] == "Unable to find the requested identifier" -> AssertionError: unexpected NotFoundException message: 'No media pipeline with the ID 00000000-0000-0000-0000-000000000000 exists.'
  The check's positive control (GetMediaPipeline against a freshly created media capture pipeline's id) is unaffected by this issue.
  Source: get_media_pipeline.rs:191-197 returns GetMediaPipelineErrorModeled::NotFoundException(crate::state::not_found(format!("No media pipeline with the ID {} exists.", input.media_pipeline_id))).

Cause 2 — a second CreateMediaPipelineKinesisVideoStreamPool with an in-use PoolName:
  Required: HTTP 400, Code "BadRequestException", Message "Media Pipeline Kinesis Video Stream Pool already exists with <PoolName> name" (fixed text around the request's PoolName), the original pool left as the only one under that name with DataRetentionInHours unchanged.
  Observed: Code "ConflictException" (HTTP 409), Message "A Kinesis Video Stream pool with that name already exists." — wrong status, wrong code, and no PoolName in the message.
  Check assertion: assert error["Code"] == "BadRequestException" -> AssertionError: expected BadRequestException for duplicate PoolName, got ConflictException.
  Source: create_media_pipeline_kinesis_video_stream_pool.rs:247-256 returns CreateMediaPipelineKinesisVideoStreamPoolErrorModeled::ConflictException(crate::state::conflict("A Kinesis Video Stream pool with that name already exists.")).

## Steps to Reproduce
Boot the emulator from this checkout and replay the behavioral suite, re-running exactly the check ids in trace_ids. Per cause:

Cause 1 (chime-sdk-media-pipelines.get-media-pipeline-not-found): call GetMediaPipeline (GET /sdk-media-pipelines/{MediaPipelineId}) with any well-formed GUID that names no existing media pipeline, e.g. MediaPipelineId="00000000-0000-0000-0000-000000000000". Expected: HTTP 404, Code "NotFoundException", Message exactly "Unable to find the requested identifier", with the requested id appearing nowhere in the body. Observed: HTTP 404 and Code "NotFoundException", but Message "No media pipeline with the ID <the id you passed> exists." Then create a media capture pipeline and GetMediaPipeline its id to confirm the success path still resolves.

Cause 2 (chime-sdk-media-pipelines.kinesis-video-stream-pool-duplicate-name-rejected): call CreateMediaPipelineKinesisVideoStreamPool(PoolName=N, StreamConfiguration={Region: <region>, DataRetentionInHours: 2}); then call it again with the same PoolName N and DataRetentionInHours: 4. Expected: HTTP 400, Code "BadRequestException", Message "Media Pipeline Kinesis Video Stream Pool already exists with N name"; then ListMediaPipelineKinesisVideoStreamPools must show exactly one pool named N, with the first pool's PoolId and DataRetentionInHours still 2. Observed: HTTP 409 ConflictException "A Kinesis Video Stream pool with that name already exists." Finally create a pool under a different name to confirm the positive control still succeeds.
