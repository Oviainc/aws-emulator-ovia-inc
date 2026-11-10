Issue-Source: trace-sweep-divergence
Issue-Trace: mediaconvert-account-metadata
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediaconvert-account-metadata mediaconvert-job-transcodes-to-complete-with-output
Issue-Shape-Id: com.amazonaws.mediaconvert#MediaConvert
Issue-Key: mediaconvert|DescribeEndpoints|2xx->2xx|$.endpoints[*].url
Issue-Key: mediaconvert|GetJob|2xx->2xx|$.job.outputGroupDetails
Issue-Key: mediaconvert|GetJob|2xx->2xx|$.job.status
Issue-Key: mediaconvert|GetJob|2xx->2xx|$.job.warnings[*]
Issue-Commit: 8ce1fb01bf2ef2be2163fc75613cfcc201e25c2b

# MediaConvert account endpoint and job completion results diverge from AWS

> _Note: this issue was identified against the code base at commit `8ce1fb01bf2ef2be2163fc75613cfcc201e25c2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEndpoints

## Description
Two net-new wire divergences in the mediaconvert emulator, both cases of the emulator answering 200 with a synthesized payload where AWS returns the real service value. BOTH causes must be closed.

Cause 1 of 2: DescribeEndpoints advertises a fabricated account-specific host that is not the address the emulator serves on (fixture mediaconvert-account-metadata, step task#1).
AWS: Smithy: DescribeEndpoints -> POST /2017-08-29/endpoints, 200, output DescribeEndpointsResponse$Endpoints (list of Endpoint), Endpoint$Url (jsonName "url", doc "URL of endpoint"); the operation and its shapes carry @deprecated "DescribeEndpoints and account specific endpoints are no longer required. We recommend that you send your requests directly to the regional endpoint instead." Recorded behavior accordingly shows endpoints[0].url being the plain regional endpoint for the region the request was made against, i.e. the same endpoint the caller is already talking to (recording: https://mediaconvert.us-west-2.amazonaws.com).
Emulator: returns 200 with endpoints[0].url pointing at a synthesized per-account subdomain of the regional host (recording: https://8b175ced.mediaconvert.us-west-2.amazonaws.com). The replay checker classifies this as host_not_own: the announced host is neither the regional endpoint nor the address this emulator is reachable on, and the random leading label resolves nowhere.
Why: the documented client flow for DescribeEndpoints is to take endpoints[0].url and re-point the client at it. A client that does so against this emulator sends every subsequent MediaConvert call to a hostname that does not exist and is not the emulator, so the whole session fails after a successful-looking DescribeEndpoints. The requirement is not a literal hostname (hosts and ports differ per environment and the generated 8-hex label is not a requirement either): endpoints[].url must be the endpoint that serves the request — the address the request arrived on / the regional endpoint form — never a fabricated account-prefixed label.
Fix: the DescribeEndpoints handler's response construction for this service crate — derive the url from the request's own endpoint/Host rather than minting an account-prefixed hostname.
Traces: mediaconvert-account-metadata#task#1 (DescribeEndpoints, 200 vs 200, $.endpoints[0].url host_not_own).

Cause 2 of 2: a valid transcode job never reaches COMPLETE and never carries outputGroupDetails or warnings (fixture mediaconvert-job-transcodes-to-complete-with-output, step task#6).
AWS: Smithy: JobStatus = SUBMITTED | PROGRESSING | COMPLETE | CANCELED | ERROR; GetJob -> GET /2017-08-29/jobs/{Id}, 200, output GetJobResponse$Job; Job$OutputGroupDetails -> list of OutputGroupDetail ("Contains details about the output groups specified in the job settings"), each with OutputDetails -> list of OutputDetail {DurationInMs "Duration in milliseconds", VideoDetails -> VideoDetail {WidthInPx, HeightInPx}}; Job$Warnings -> list of WarningGroup with required members Code ("Warning code that identifies a specific warning in the job") and Count ("The number of times this warning occurred in the job"). A job whose settings describe a processable transcode therefore polls SUBMITTED -> PROGRESSING -> COMPLETE, and in the COMPLETE projection carries one outputGroupDetails entry per output group with per-output durationInMs and videoDetails width/height taken from the produced output, plus a warnings entry {code,count} for each warning class the transcode raised (recorded behavior only for the specific codes: this fixture's input yields codes 230001 and 230005, count 1 each).
Emulator: GetJob answers 200 but job.status is "ERROR" and stays ERROR — the fixture's poll aborts because ERROR is in fail_on after 2 attempts — and job.outputGroupDetails and job.warnings are absent entirely.
Why: the normal MediaConvert workflow is CreateJob followed by polling GetJob until COMPLETE. Against this emulator every such client sees a hard terminal failure seconds after submission, can never branch on PROGRESSING, and can never read the output geometry/duration it is polling for; the downstream artifact is likewise never produced (the companion step reports the destination prefix empty, which is already owned by the s3 ListObjectsV2 issue 20261029T192819Z and is not re-filed here).
Fix: the job state machine in the service's state layer (the settle/transition path that terminates jobs) plus the Job projection/serializer that builds the GetJob view — a processable job must transition to COMPLETE and the view must populate outputGroupDetails (and warnings when the transcode raises them) instead of hardcoding them absent.
Related open issue, which does NOT cover this step: 20261024T051247Z-mediaconvert-getjob-wrong-behavior.md ("Every submitted job terminates in ERROR ~750ms after CreateJob") describes the same code path (state.rs settle_job forcing JOB_STATUS_ERROR, JobRecord::view hardcoding output_group_details: None) but carries no fixture replay key and says nothing about Job$Warnings, so this fixture step is ungated by it; closing this cause requires re-replaying the trace named below.
Traces: mediaconvert-job-transcodes-to-complete-with-output#task#6 (GetJob, 200 vs 200, $.job.status COMPLETE vs ERROR, $.job.outputGroupDetails and $.job.warnings[0..1] missing).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200; 77 steps, 68 passed, score 88.3%).

Cause 1 — fixture mediaconvert-account-metadata, step task#1, operation DescribeEndpoints, verdict mismatch, expected_status 200, actual_status 200:
  $.endpoints[0].url: host_not_own (expected 'https://mediaconvert.us-west-2.amazonaws.com', got 'https://8b175ced.mediaconvert.us-west-2.amazonaws.com')
The leading 8-hex label is emulator-generated and is not the address the emulator answers on.

Cause 2 — fixture mediaconvert-job-transcodes-to-complete-with-output, step task#6, operation GetJob, verdict mismatch, expected_status 200, actual_status 200:
  $.job.status: value_mismatch (expected 'COMPLETE', got 'ERROR')
  $.job.status: poll: 'ERROR' is in fail_on after 2 attempt(s)
  $.job.outputGroupDetails: missing (expected [{'outputDetails': [{'durationInMs': 2033, 'videoDetails': {'widthInPx': 1280, 'heightInPx': 720}}]}], got None)
  $.job.warnings[0]: missing (expected {'code': 230001, 'count': 1}, got None)
  $.job.warnings[1]: missing (expected {'code': 230005, 'count': 1}, got None)
The recorded durations, pixel dimensions and warning codes are this fixture's input-specific values; the requirement is that the members are present and derived from the job's inputs/outputs, not these literals.

## Steps to Reproduce
Cause 1:
1. Boot the emulator and point a mediaconvert client at its data listener.
2. Call DescribeEndpoints (POST /2017-08-29/endpoints, empty body).
3. Observe endpoints[0].url = https://<random-hex>.mediaconvert.<region>.amazonaws.com. Expected: the regional endpoint that serves the request (the address the client just reached), with no fabricated account label.
4. Re-point the client at the returned url and issue any MediaConvert call: it cannot connect, because the announced host is not the emulator.
5. Or replay fixture mediaconvert-account-metadata, step task#1.

Cause 2:
1. Boot the emulator; create an S3 bucket with an input object and a queue.
2. CreateJob with one FILE_GROUP_SETTINGS output group describing a valid H.264/MP4 transcode of that input.
3. Poll GetJob(Id=<id>) until a terminal status.
4. Observe status 'ERROR' with no job.outputGroupDetails and no job.warnings. Expected: the job leaves SUBMITTED through PROGRESSING and settles on COMPLETE with one outputGroupDetails entry per output group (outputDetails[].durationInMs, videoDetails.widthInPx/heightInPx) and warnings[] entries {code,count} for any warnings raised.
5. Or replay fixture mediaconvert-job-transcodes-to-complete-with-output, step task#6.
