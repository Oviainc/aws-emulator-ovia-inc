Issue-Source: trace-sweep-divergence
Issue-Trace: polly-speech-synthesis-task-lifecycle
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: polly-speech-synthesis-task-lifecycle
Issue-Shape-Id: com.amazonaws.polly#Parrot_v1
Issue-Key: polly|StartSpeechSynthesisTask|2xx->2xx|$.SynthesisTask.SdkFeatureToggles
Issue-Commit: d36bb525c2331ae923943e9e5ae02c5704a8c1d0

# Polly StartSpeechSynthesisTask omits the SynthesisTask.SdkFeatureToggles member from its response

> _Note: this issue was identified against the code base at commit `d36bb525c2331ae923943e9e5ae02c5704a8c1d0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSpeechSynthesisTask

## Description
StartSpeechSynthesisTask's 200 response body is missing one member of the SynthesisTask object that Polly actually puts on the wire. 1 cause.

Cause 1 of 1: the SynthesisTask returned by StartSpeechSynthesisTask has no SdkFeatureToggles member.
AWS: recorded behavior only. The service's Smithy model (services/service-com-amazonaws-polly-parrot-v1/smithy-model/com-amazonaws-polly-parrot-v1.json) does NOT declare this member — smithy_query(query="shape", shape="SynthesisTask,StartSpeechSynthesisTaskOutput") lists exactly Engine, TaskId, TaskStatus, TaskStatusReason, OutputUri, CreationTime, RequestCharacters, SnsTopicArn, LexiconNames, OutputFormat, SampleRate, SpeechMarkTypes, TextType, VoiceId and LanguageCode, and StartSpeechSynthesisTaskOutput carries only SynthesisTask. The recording nevertheless shows the real service serializing an additional member on the StartSpeechSynthesisTask response: SynthesisTask.SdkFeatureToggles, present as an EMPTY JSON array. The member name and the fact that it is serialized even when it has no entries are the fixed part; its contents are empty for an ordinary task. The response status is 200 in both the recording and the emulator, so this is purely a body-shape divergence and no error code or message is involved. Hypothesis: the member is an unmodeled, forward-compatibility field the service emits unconditionally on the task-creation response; the corpus's GetSpeechSynthesisTask and ListSpeechSynthesisTasks steps in the same fixture PASS, so it is the StartSpeechSynthesisTask response specifically that must carry it, not every SynthesisTask serialization.
Emulator: the member is absent entirely — the replayed step reports `$.SynthesisTask.SdkFeatureToggles: missing (expected [], got None)`. Every other member of the returned SynthesisTask matches, and the status matches at 200.
Why: a client that compares the task-creation response against a recorded or golden AWS body, or that round-trips the body through a strict/exhaustive JSON decoder (non-SDK clients, contract tests, response-diffing proxies and record-and-replay harnesses built on real Polly captures), sees a structurally different object for an otherwise successful call. Because the member is absent rather than null, code that keys on its presence to distinguish a Polly-generated task envelope from a locally constructed one takes the wrong branch, and a byte/shape-level assertion on the task-creation response fails even though the task itself was accepted correctly.
Fix: the StartSpeechSynthesisTask response serializer — the SynthesisTask body construction in the operation's handler (services/service-com-amazonaws-polly-parrot-v1/src/ops/start_speech_synthesis_task.rs, which builds the task record and the 200 output alongside output_uri()). It must emit SdkFeatureToggles as an empty array on the StartSpeechSynthesisTask response. Scope it to that response only: GetSpeechSynthesisTask and ListSpeechSynthesisTasks replay clean in the same fixture, so adding the member to the shared SynthesisTask serialization used by those operations would introduce new divergences there.
Traces: polly-speech-synthesis-task-lifecycle#task#2 (StartSpeechSynthesisTask, 200 -> 200, `$.SynthesisTask.SdkFeatureToggles: missing (expected [], got None)`).

Not covered by any open issue: no open polly issue carries this step's replay key. The nearest open issues are 20261102T121012Z-polly-asynchronous-synthesis-tasks-have-no-side-effects.md (behavioral: no object written to the output bucket, no SNS notification, ListSpeechSynthesisTasks ordering — nothing about the response body's members), 20261022T175007Z-polly-startspeechsynthesistask-stub.md (the operation is implemented now and answers 200, so that stub key does not reproduce here) and 20261022T232910Z-polly-startspeechsynthesistask-performs-no-request-validation-lexicon-speech.md (request rejections, not the success-path body).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, data listener :42099; 46 steps, 38 passed, score 82.6%).

Divergence cluster (verdict=mismatch, service=polly, operation=StartSpeechSynthesisTask):
  fixture_id: polly-speech-synthesis-task-lifecycle
  step_key:   task#2
  expected_status: 200, actual_status: 200
  detail: $.SynthesisTask.SdkFeatureToggles: missing (expected [], got None)
  divergence_key: service=polly|StartSpeechSynthesisTask|status=200->200|keys=$.SynthesisTask.SdkFeatureToggles
  affected (1): polly-speech-synthesis-task-lifecycle#task#2
  stub_path: (none), owned_by: (none)

Expected vs actual, as the step reports it:
  expected: SynthesisTask.SdkFeatureToggles = []  (member present, empty array)
  actual:   SynthesisTask.SdkFeatureToggles absent from the response body
No other member of the SynthesisTask object diverged in this step, and the HTTP status agreed at 200.

Model facts used: smithy_query(query="shape", shape="SynthesisTask,StartSpeechSynthesisTaskOutput", model_path="services/service-com-amazonaws-polly-parrot-v1/smithy-model/com-amazonaws-polly-parrot-v1.json") — SdkFeatureToggles is not a modeled member, hence "recorded behavior only".
Source corroboration: services/service-com-amazonaws-polly-parrot-v1/src/ops/start_speech_synthesis_task.rs (task record + 200 output construction, output_uri() at lines 279-289).

## Steps to Reproduce
1. Boot the emulator from this checkout so the polly service is serving.
2. Create an S3 bucket for the task output, then call StartSpeechSynthesisTask with OutputFormat=mp3, VoiceId=Joanna, OutputS3BucketName=<bucket> and a short Text.
3. Inspect the raw 200 response body (not an SDK-deserialized object — the member is unmodeled, so SDKs discard it). AWS returns a SynthesisTask object that includes a SdkFeatureToggles member serialized as an empty array `[]`; the emulator returns the same object with that member entirely absent.
4. Re-replay fixture polly-speech-synthesis-task-lifecycle, step task#2, and confirm $.SynthesisTask.SdkFeatureToggles is present and equal to [].
