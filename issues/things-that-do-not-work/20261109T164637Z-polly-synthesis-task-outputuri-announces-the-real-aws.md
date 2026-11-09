Issue-Source: trace-sweep-divergence
Issue-Trace: polly-speech-synthesis-task-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: polly-speech-synthesis-task-lifecycle
Issue-Shape-Id: com.amazonaws.polly#Parrot_v1
Issue-Key: polly|StartSpeechSynthesisTask|2xx->2xx|$.SynthesisTask.OutputUri
Issue-Key: polly|GetSpeechSynthesisTask|2xx->2xx|$.SynthesisTask.OutputUri
Issue-Key: polly|ListSpeechSynthesisTasks|2xx->2xx|$.SynthesisTasks[*].OutputUri
Issue-Related: 20261103T042343Z-polly-startspeechsynthesistask-omits-the-synthesistask-sdkfeaturetoggles-member-from.md
Issue-Commit: 6c5e636bebc82ccd4609eb5d54ba0275e4ea7ff4

# Polly synthesis-task OutputUri announces the real AWS S3 endpoint instead of the emulator's own address

> _Note: this issue was identified against the code base at commit `6c5e636bebc82ccd4609eb5d54ba0275e4ea7ff4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSpeechSynthesisTask

## Description
Every operation that serializes a SynthesisTask reports an OutputUri whose host is the real AWS S3 endpoint for the request's Region rather than the address the emulator is reached on, so the URI the task hands back points off-box. The replay flags it as `host_not_own` on all three task operations of fixture polly-speech-synthesis-task-lifecycle. 3 causes, one per operation that serializes the task envelope; they share one root cause, the URI builder in the StartSpeechSynthesisTask handler.

Cause 1 of 3: StartSpeechSynthesisTask's 200 response reports an OutputUri on a host the emulator does not serve (fixture polly-speech-synthesis-task-lifecycle, step task#2).
AWS: Smithy (services/service-com-amazonaws-polly-parrot-v1/smithy-model/com-amazonaws-polly-parrot-v1.json) models StartSpeechSynthesisTask as POST /v1/synthesisTasks -> 200 with output SynthesisTask; SynthesisTask$OutputUri is documented "Pathway for the output speech file", and OutputS3BucketName (@required, pattern ^[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9]$) plus OutputS3KeyPrefix name where the object is written. Recorded behavior: the URI is an S3 object URL whose fixed parts are the scheme, the S3 endpoint for the Region the call was made against, and the path `/<bucket>/<key-prefix><task-id>.<format-suffix>`; the bucket, the key prefix, the task id and the format suffix are the variable parts, taken from the request and the minted task. The requirement is that the endpoint be the S3 endpoint the CALLER is talking to, which in a recording against real AWS is the AWS endpoint and in this emulator is the emulator's own S3 address. Status is 200 on both sides, so no error code or message is involved.
Emulator: the URI is built with the AWS production host hardcoded — services/service-com-amazonaws-polly-parrot-v1/src/ops/start_speech_synthesis_task.rs lines 279-289, `format!("https://s3.{region}.amazonaws.com/{bucket}/{prefix}{task_id}.{suffix}")` — so the announced host is never the host that served the request. The replay reports `$.SynthesisTask.OutputUri: host_not_own`. The same step also reports `$.SynthesisTask.SdkFeatureToggles: missing (expected [], got None)`; that member is already owned by the open issue 20261103T042343Z-polly-startspeechsynthesistask-omits-the-synthesistask-sdkfeaturetoggles-member-from.md and is NOT re-filed here, but note the step only replays clean once both that member and this host are fixed (the cluster's replay key now covers both keys, which is why this step came back unowned).
Why: a client that polls the task and then fetches OutputUri is sent to the public AWS S3 endpoint instead of the emulator. In a sealed test environment the fetch fails on name resolution or egress and looks like a broken bucket or broken credentials; where egress is allowed it is worse, because the client reaches real S3 with test credentials and gets 403/NoSuchBucket for a bucket that only exists in the emulator. Any pipeline that passes OutputUri to a downstream consumer propagates an address that cannot be read back.
Fix: the OutputUri construction in the StartSpeechSynthesisTask handler (services/service-com-amazonaws-polly-parrot-v1/src/ops/start_speech_synthesis_task.rs, output_uri()) must derive the S3 endpoint from the address the emulator serves on (the same endpoint its own S3 service answers), not from a hardcoded amazonaws.com host, keeping the `/<bucket>/<prefix><task-id>.<suffix>` path exactly as it is built today.
Traces: polly-speech-synthesis-task-lifecycle#task#2 (StartSpeechSynthesisTask, 200 -> 200, `$.SynthesisTask.OutputUri: host_not_own`).

Cause 2 of 3: GetSpeechSynthesisTask replays the same off-box host back to the caller (fixture polly-speech-synthesis-task-lifecycle, step task#3).
AWS: Smithy models GetSpeechSynthesisTask as GET /v1/synthesisTasks/{TaskId} -> 200 with output SynthesisTask (errors InvalidTaskIdException 400, SynthesisTaskNotFoundException 400, ServiceFailureException 500); recorded behavior: the polled task reports the same OutputUri the creation call reported, on the S3 endpoint the caller is talking to.
Emulator: the stored task's output_uri — built once by output_uri() above — is returned verbatim, so the polling response carries the AWS production host too; the replay reports `$.SynthesisTask.OutputUri: host_not_own` with status agreeing at 200.
Why: polling to `completed` and then reading OutputUri is the normal way to locate the result, so this is the path on which a client actually dereferences the bad address.
Fix: fixing output_uri() at the point the task is minted repairs this response, since GetSpeechSynthesisTask's serializer echoes the stored value; if the value is instead rendered at read time, that rendering must use the emulator's own S3 endpoint as well.
Traces: polly-speech-synthesis-task-lifecycle#task#3 (GetSpeechSynthesisTask, 200 -> 200, `$.SynthesisTask.OutputUri: host_not_own`).

Cause 3 of 3: ListSpeechSynthesisTasks lists tasks whose OutputUri carries the same off-box host (fixture polly-speech-synthesis-task-lifecycle, step task#4).
AWS: Smithy models ListSpeechSynthesisTasks as GET /v1/synthesisTasks -> 200, @paginated, with SynthesisTasks + NextToken; recorded behavior: each listed task's OutputUri is the same S3 object URL on the endpoint the caller is talking to.
Emulator: `$.SynthesisTasks[0].OutputUri: host_not_own` — the listing serializes the stored URI unchanged, so every listed task advertises the AWS production host; status agrees at 200.
Why: clients that enumerate tasks to collect their outputs (batch collectors, dashboards) get a whole page of unreachable addresses, not just one.
Fix: same component as Cause 1 — the task's OutputUri must be built against the emulator's own S3 endpoint, so every serialization of the task envelope (start, get, list) reports a host the emulator serves.
Traces: polly-speech-synthesis-task-lifecycle#task#4 (ListSpeechSynthesisTasks, 200 -> 200, `$.SynthesisTasks[0].OutputUri: host_not_own`).

Not covered by any open issue: 20261102T121012Z-polly-asynchronous-synthesis-tasks-have-no-side-effects.md is about the output object never being written, the missing SNS notification and the listing order — it says nothing about the URI's host; 20261103T042343Z covers only the SdkFeatureToggles member of task#2; 20261022T175007Z and 20261022T232910Z concern the (now implemented) stub and request validation.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, /__admin/health 200, data listener :34167; 46 steps, 37 passed, 9 mismatched, score 80.4%).

Cluster 1 (verdict=mismatch, service=polly, operation=StartSpeechSynthesisTask):
  fixture_id: polly-speech-synthesis-task-lifecycle, step_key: task#2
  expected_status 200, actual_status 200
  detail: $.SynthesisTask.SdkFeatureToggles: missing (expected [], got None)
          $.SynthesisTask.OutputUri: host_not_own (expected an S3 object URL on the caller's S3 endpoint, got 'https://s3.us-west-2.amazonaws.com/<bucket>/<task-id>.mp3')
  divergence_key: service=polly|StartSpeechSynthesisTask|status=200->200|keys=$.SynthesisTask.OutputUri,$.SynthesisTask.SdkFeatureToggles
  affected (1): polly-speech-synthesis-task-lifecycle#task#2 — stub_path: none, owned_by: none

Cluster 2 (verdict=mismatch, operation=GetSpeechSynthesisTask):
  fixture_id: polly-speech-synthesis-task-lifecycle, step_key: task#3
  expected_status 200, actual_status 200
  detail: $.SynthesisTask.OutputUri: host_not_own
  divergence_key: service=polly|GetSpeechSynthesisTask|status=200->200|keys=$.SynthesisTask.OutputUri
  affected (1): polly-speech-synthesis-task-lifecycle#task#3 — stub_path: none, owned_by: none

Cluster 3 (verdict=mismatch, operation=ListSpeechSynthesisTasks):
  fixture_id: polly-speech-synthesis-task-lifecycle, step_key: task#4
  expected_status 200, actual_status 200
  detail: $.SynthesisTasks[0].OutputUri: host_not_own
  divergence_key: service=polly|ListSpeechSynthesisTasks|status=200->200|keys=$.SynthesisTasks[0].OutputUri
  affected (1): polly-speech-synthesis-task-lifecycle#task#4 — stub_path: none, owned_by: none

Expected vs actual, as the steps report it:
  expected: SynthesisTask.OutputUri = an S3 object URL whose host is the S3 endpoint the caller reached, path /<bucket>/<key-prefix><task-id>.<format-suffix>
  actual:   the path is correct, but the host is always the AWS production endpoint s3.<region>.amazonaws.com — a host the emulator does not serve, which is why the comparator's verdict is host_not_own even where the two strings are textually equal (the recording was made against real AWS, where that host IS the caller's endpoint).
The per-task UUID and the bucket name in the recorded URI are per-run values and are not part of the requirement.

Source corroboration: services/service-com-amazonaws-polly-parrot-v1/src/ops/start_speech_synthesis_task.rs lines 271-289 — output_uri() returns format!("https://s3.{region}.amazonaws.com/{bucket}/{prefix}{task_id}.{suffix}").
Model facts used: smithy_query(query="operation", operation="StartSpeechSynthesisTask,GetSpeechSynthesisTask,ListSpeechSynthesisTasks", model_path="services/service-com-amazonaws-polly-parrot-v1/smithy-model/com-amazonaws-polly-parrot-v1.json") — the model constrains the bucket/prefix inputs and documents OutputUri as the "Pathway for the output speech file" but does not spell the host, hence "recorded behavior only" for the endpoint rule.

## Steps to Reproduce
1. Boot the emulator from this checkout (polly and s3) and note the address the data listener serves on.
2. Create an S3 bucket, then call StartSpeechSynthesisTask with OutputFormat=mp3, VoiceId=Joanna, OutputS3BucketName=<bucket> and a short Text. Read SynthesisTask.OutputUri from the raw 200 body: its host is s3.<region>.amazonaws.com, i.e. the public AWS endpoint, not the emulator's S3 address.
3. Poll GetSpeechSynthesisTask with that TaskId and call ListSpeechSynthesisTasks: both responses report the same off-box host in SynthesisTask.OutputUri / SynthesisTasks[].OutputUri.
4. Try to fetch OutputUri with a plain HTTP client configured only against the emulator: the request leaves for (or is refused on the way to) real AWS instead of reaching the emulator's S3 service.
5. Re-replay fixture polly-speech-synthesis-task-lifecycle steps task#2, task#3 and task#4 and confirm no step reports host_not_own on OutputUri.
