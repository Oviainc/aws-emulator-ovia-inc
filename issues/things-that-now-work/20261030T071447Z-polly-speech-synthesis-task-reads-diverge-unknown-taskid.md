Issue-Source: trace-sweep-divergence
Issue-Trace: polly-speech-synthesis-task-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: polly-speech-synthesis-task-errors polly-speech-synthesis-task-lifecycle
Issue-Shape-Id: com.amazonaws.polly#Parrot_v1
Issue-Key: polly|GetSpeechSynthesisTask|4xx->4xx|$.message
Issue-Key: polly|GetSpeechSynthesisTask|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: polly|ListSpeechSynthesisTasks|2xx->2xx|$.SynthesisTasks[*]
Issue-Commit: 533efece7fa11c0e537273065624bf5d01aa80f8

# Polly speech-synthesis task reads diverge: unknown TaskId answers InvalidTaskIdException and ListSpeechSynthesisTasks reports no tasks (2 causes)

> _Note: this issue was identified against the code base at commit `533efece7fa11c0e537273065624bf5d01aa80f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSpeechSynthesisTask

## Description
Two distinct causes in the synthesis-task read path: the not-found classification of GetSpeechSynthesisTask, and ListSpeechSynthesisTasks not reporting the tasks the service holds.

Cause 1 of 2 (fixture polly-speech-synthesis-task-errors, step task#1 — GetSpeechSynthesisTask): a TaskId that satisfies the modeled pattern but names no held task is refused as an INVALID id instead of as a MISSING task, and the error body/header do not carry the shape AWS reports.
AWS: Smithy (services/service-com-amazonaws-polly-parrot-v1/smithy-model/com-amazonaws-polly-parrot-v1.json) declares GetSpeechSynthesisTaskInput$TaskId as @required @httpLabel with @pattern ^[a-zA-Z0-9_-]{1,100}$, and binds two client errors at HTTP 400: SynthesisTaskNotFoundException, documented "The Speech Synthesis task with requested Task ID cannot be found.", and InvalidTaskIdException, documented "The provided Task ID is not valid. Please provide a valid Task ID and try again." An id that SATISFIES the member pattern and merely names no task is therefore the not-found case. Recorded behavior: HTTP 400 with x-amzn-errortype naming the error shape followed by this service's error namespace suffix — the shape name is the variable part taken from the error that occurred, the ":http://polly.amazonaws.com/doc/v1" suffix is fixed text the Polly error serializer always appends — and with NO `message` key in the body at all (the recording's message is absent, not an empty string).
Emulator: HTTP 400 (status is right) with x-amzn-errortype = "InvalidTaskIdException" — the wrong error shape AND without the ":http://polly.amazonaws.com/doc/v1" namespace suffix — and a body that DOES carry message = "The provided Task ID is not valid. Please provide a valid Task ID and try again." where the recording has no message key. So an unknown-but-well-formed TaskId is mis-classified as malformed, and the serializer adds a message the recorded error class does not carry.
Why: both errors are HTTP 400, so only the code distinguishes "this id is garbage, do not retry" from "this task is gone or expired, re-start the synthesis" — a client branching on the code takes the wrong branch for every well-formed unknown id. The truncated errortype header additionally breaks SDKs/proxies that match the fully-qualified errortype value AWS sends, and the extra message key makes a client that tests for the presence of message see a different error contract.
Fix: the request classification in this operation's handler (services/service-com-amazonaws-polly-parrot-v1/src/ops/get_speech_synthesis_task.rs, which gates on crate::state::is_task_id from src/state.rs) must let a pattern-valid id reach the store lookup and answer SynthesisTaskNotFoundException when it names nothing; and this service's error serializer must emit the errortype header with the service's error-namespace suffix and must not synthesize a message for this error class.
Related open issue: 20261029T203859Z-polly-getspeechsynthesistask-refuses-every-non-uuid-taskid-with-invalidtaskidexception.md reports the same mis-classification from the BEHAVIORAL suite and carries no wire replay key for this fixture, and it does not cover the errortype-header suffix or the extra message key; this entry carries the wire step so the fix can be re-replayed against it.
Traces: polly-speech-synthesis-task-errors#task#1.

Cause 2 of 2 (fixture polly-speech-synthesis-task-lifecycle, step task#4 — ListSpeechSynthesisTasks): the listing answers 200 with an empty SynthesisTasks list where the recording reports the synthesis task created earlier in the same fixture.
AWS: Smithy models ListSpeechSynthesisTasks as GET /v1/synthesisTasks -> 200 with SynthesisTasks: list<SynthesisTask> and NextToken, @paginated (MaxResults 1..100, Status filter). A task started with StartSpeechSynthesisTask is held by the service and reported by this listing with its full record: Engine, TaskId, TaskStatus, OutputFormat, OutputUri, CreationTime, RequestCharacters, TextType, VoiceId, and the optional members omitted/null when unset. Recorded behavior: after a task completes, the listing's first element is that task with TaskStatus "completed", the OutputUri pointing at the configured output bucket object for its TaskId, and RequestCharacters equal to the synthesized text's billable character count.
Emulator: the same request returns 200 with no element at SynthesisTasks[0] — the created task is not reported at all, so the listing is empty where AWS lists one task.
Why: the whole asynchronous-synthesis workflow is driven by listing/polling tasks; a client that starts a task and then lists or polls for it concludes the task does not exist, and never retrieves its OutputUri. Hypothesis: the task record is never persisted into the listing's view because the creating operation is still a scaffold stub (StartSpeechSynthesisTask, tracked by the already-filed 20261022T175007Z-polly-startspeechsynthesistask-stub.md), so this listing cannot be confirmed correct until that handler writes into the task store; the listing must report every held task once it does.
Fix: the synthesis-task store this service keeps (src/state.rs) and the ListSpeechSynthesisTasks handler that projects it (src/ops/list_speech_synthesis_tasks.rs), so a created task is reported with its full SynthesisTask record; gated on StartSpeechSynthesisTask persisting the task. The open issue 20261029T065855Z-polly-listspeechsynthesistasks-pagination--r2.md concerns NextToken acceptance, not the listing's contents, and does not cover this.
Traces: polly-speech-synthesis-task-lifecycle#task#4.

## Evidence
Cause 1 (polly-speech-synthesis-task-errors#task#1, GetSpeechSynthesisTask): expected HTTP 400, actual HTTP 400; $headers.x-amzn-errortype value_mismatch — expected "SynthesisTaskNotFoundException:http://polly.amazonaws.com/doc/v1", got "InvalidTaskIdException"; $.message type_mismatch — expected None (no message key), got "The provided Task ID is not valid. Please provide a valid Task ID and try again.".
Cause 2 (polly-speech-synthesis-task-lifecycle#task#4, ListSpeechSynthesisTasks): expected HTTP 200, actual HTTP 200; $.SynthesisTasks[0] missing — the recording carries a SynthesisTask record {Engine: standard, OutputFormat: mp3, OutputUri: <output-bucket object for the task id>, RequestCharacters: 76, TaskId: <the task minted earlier in the fixture>, TaskStatus: completed, TextType: text, VoiceId: Matthew, CreationTime shaped-not-asserted, remaining members null}; the emulator returns None at that position.
Model facts used: smithy_query(operation="GetSpeechSynthesisTask,ListSpeechSynthesisTasks") -> GetSpeechSynthesisTask GET /v1/synthesisTasks/{TaskId}, TaskId @required @httpLabel pattern ^[a-zA-Z0-9_-]{1,100}$, errors InvalidTaskIdException(400), SynthesisTaskNotFoundException(400), ServiceFailureException(500); ListSpeechSynthesisTasks GET /v1/synthesisTasks -> 200, output SynthesisTasks + NextToken, paginated. smithy_query(shape="SynthesisTaskNotFoundException,InvalidTaskIdException,SynthesisTask") for the two error docs and the task record's members.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture polly-speech-synthesis-task-errors step task#1: GET /v1/synthesisTasks/<a pattern-valid TaskId that names no task>. The emulator answers 400 with x-amzn-errortype "InvalidTaskIdException" and a message body; it must answer 400 with x-amzn-errortype "SynthesisTaskNotFoundException:http://polly.amazonaws.com/doc/v1" and no message key.
3. Confirm the malformed-id layer is unchanged: a TaskId violating ^[a-zA-Z0-9_-]{1,100}$ must still be refused at the request boundary with the pattern ValidationException.
4. Replay fixture polly-speech-synthesis-task-lifecycle through step task#4: start a synthesis task, let it complete, then GET /v1/synthesisTasks. The emulator returns 200 with an empty SynthesisTasks list; it must list the created task with its TaskId, TaskStatus completed, OutputUri, OutputFormat, Engine, TextType, VoiceId and RequestCharacters.
5. Re-replay both steps to confirm.
