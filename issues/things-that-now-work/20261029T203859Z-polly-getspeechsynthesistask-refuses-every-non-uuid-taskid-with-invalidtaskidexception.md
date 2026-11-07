Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: polly.get-speech-synthesis-task-not-found-rejected polly.get-speech-synthesis-task-invalid-id-format-rejected
Issue-Shape-Id: com.amazonaws.polly#Parrot_v1
Issue-Key: polly.get-speech-synthesis-task-not-found-rejected|run#10|AssertionError|assert code_missing == "SynthesisTaskNotFoundException", code_missing
Issue-Key: polly.get-speech-synthesis-task-invalid-id-format-rejected|run#9|AssertionError|assert unknown_code == "SynthesisTaskNotFoundException", unknown_code
Issue-Commit: 6074294cbfccbc2257fa0c432afede02752270ab

# GetSpeechSynthesisTask refuses every non-UUID TaskId with InvalidTaskIdException, so a well-formed unknown TaskId never answers SynthesisTaskNotFoundException

> _Note: this issue was identified against the code base at commit `6074294cbfccbc2257fa0c432afede02752270ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSpeechSynthesisTask

## Description
REOPENS 20261024T133856Z-polly-getspeechsynthesistask-error-response.md: its fix left the call refused.

Cause 1 of 1: the TaskId classifier is too narrow, so the not-found layer of GetSpeechSynthesisTask is unreachable for any pattern-valid id that is not a canonical hyphenated UUID.

AWS: Smithy (services/service-com-amazonaws-polly-parrot-v1/smithy-model/com-amazonaws-polly-parrot-v1.json) declares GetSpeechSynthesisTaskInput$TaskId with @pattern ^[a-zA-Z0-9_-]{1,100}$ and @required, and the operation declares two client errors at HTTP 400: InvalidTaskIdException ("The provided Task ID is not valid. Please provide a valid Task ID and try again.") and SynthesisTaskNotFoundException ("The Speech Synthesis task with requested Task ID cannot be found."). Recorded behavior shows three distinct layers: (a) a TaskId that violates the member pattern is refused at the request-shape boundary with HTTP 400 ValidationException and the message `1 validation error detected: Value '<taskId>' at 'taskId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_-]{1,100}$` — fixed text is everything but `<taskId>`, which is the rejected value taken verbatim from the request; the emulator gets this layer right. (b) a TaskId that SATISFIES the member pattern but names no task the service holds is refused with HTTP 400 SynthesisTaskNotFoundException, whose body carries `message` as JSON null. The recordings exercise this with a 40-character pattern-valid id that is NOT UUID-shaped, so SynthesisTaskNotFoundException is not reserved for UUID-shaped ids: any pattern-valid id that is merely unknown lands here. (c) a TaskId naming a real, just-created task is found and returned (positive control).

Emulator: services/service-com-amazonaws-polly-parrot-v1/src/ops/get_speech_synthesis_task.rs lines 159-167 judges the id BEFORE any store lookup with crate::state::is_task_id, defined at src/state.rs lines 450-452 as `task_id.len() == HYPHENATED_UUID_LENGTH && ::uuid::Uuid::try_parse(task_id).is_ok()`. Any pattern-valid id that is not a 36-character canonical hyphenated UUID is refused with HTTP 400 InvalidTaskIdException instead of reaching the lookup. Replay: GetSpeechSynthesisTask with TaskId of 40 'a' characters answers error code `InvalidTaskIdException` where the recording answers `SynthesisTaskNotFoundException`. The not-found branch at lines 181-191 is therefore dead for every non-UUID-shaped id, and the clock-driven retention path behind it is never exercised by such a request.

Why: a client that branches on the error code to tell "this id is garbage, do not retry" from "this task is gone or expired, re-start the synthesis" takes the wrong branch for every pattern-valid id that is not UUID-shaped. Because both errors are HTTP 400, only the code distinguishes them, so the mis-classification is invisible at the status level. The earlier fix (20261024T133856Z) correctly made InvalidTaskIdException reachable but drew the boundary at "is a canonical UUID" rather than at "is the minted form" vs "is unknown", which over-refuses: the recordings show a pattern-valid, non-UUID, unknown id is a NOT-FOUND, not an INVALID id.

Fix: belongs in this operation's request classification — the is_task_id gate in the GetSpeechSynthesisTask handler and/or its definition in the service state helper. The pre-lookup refusal must not reject every pattern-valid non-UUID id; a pattern-valid id that names no held task has to fall through to the SynthesisTaskNotFoundException branch (with `message` serialized as JSON null, per the already-fixed 20261022T232910Z-polly-getspeechsynthesistask-synthesistasknotfoundexception-body-omits-the-message-key.md). Any InvalidTaskIdException path kept must be narrowed to inputs the recordings actually show refused that way, not to "not a UUID".

Traces: polly.get-speech-synthesis-task-not-found-rejected (assertion at check line 40), polly.get-speech-synthesis-task-invalid-id-format-rejected (assertion at check line 42). Both pass layer (a) and fail layer (b); neither reaches layer (c), whose StartSpeechSynthesisTask dependency is covered separately by 20261022T175007Z-polly-startspeechsynthesistask-stub.md.

## Evidence
Expected (recorded AWS): GetSpeechSynthesisTask(TaskId="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa") -> HTTP 400, Error.Code = "SynthesisTaskNotFoundException", Error.Message absent/null.
Actual (emulator): HTTP 400, Error.Code = "InvalidTaskIdException".
Replay output: `AssertionError: InvalidTaskIdException` raised at `assert code_missing == "SynthesisTaskNotFoundException", code_missing` (check_polly_get_speech_synthesis_task_not_found_rejected.py:40) and at `assert unknown_code == "SynthesisTaskNotFoundException", unknown_code` (check_polly_get_speech_synthesis_task_invalid_id_format_rejected.py:42).
The malformed-id leg of both checks PASSES: ValidationException with `1 validation error detected: Value '<taskId>' at 'taskId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_-]{1,100}$` is already correct, which localizes the divergence to the post-boundary classification.
Code: src/ops/get_speech_synthesis_task.rs:162-167 returns InvalidTaskIdException whenever `!crate::state::is_task_id(&input.task_id)`; src/state.rs:450-452 defines is_task_id as canonical-hyphenated-UUID-only.
Model: smithy_query(operation="GetSpeechSynthesisTask") -> errors InvalidTaskIdException (400) and SynthesisTaskNotFoundException (400); input member TaskId pattern ^[a-zA-Z0-9_-]{1,100}$, httpLabel, required.

## Steps to Reproduce
1. Boot the emulator and call GET /v1/synthesisTasks/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa (40 'a' characters: satisfies the modeled TaskId pattern, so the boundary validator admits it; not a hyphenated UUID, so is_task_id rejects it).
2. Observe HTTP 400 with Error.Code = "InvalidTaskIdException".
3. Real Polly answers HTTP 400 "SynthesisTaskNotFoundException" with a null message for this request: the id is well-formed and merely names no task.
4. Confirm the boundary layer is unaffected: GetSpeechSynthesisTask with TaskId = "bad id! not/valid" must still answer ValidationException with the regular-expression-pattern message.
5. Re-replay polly.get-speech-synthesis-task-not-found-rejected and polly.get-speech-synthesis-task-invalid-id-format-rejected to confirm the fix (their positive-control leg additionally needs StartSpeechSynthesisTask, tracked by 20261022T175007Z-polly-startspeechsynthesistask-stub.md).
