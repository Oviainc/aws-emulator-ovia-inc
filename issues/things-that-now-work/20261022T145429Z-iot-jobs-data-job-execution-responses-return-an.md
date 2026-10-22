Issue-Source: trace-sweep-divergence
Issue-Trace: iotjobsdata-absent-thing-permissive-queue-reads
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotjobsdata-absent-thing-permissive-queue-reads iotjobsdata-job-execution-update-and-terminal-refusals
Issue-Shape-Id: com.amazonaws.iotjobsdataplane#IotLaserThingJobManagerExternalService
Issue-Key: iot-jobs-data|StartNextPendingJobExecution|2xx->2xx|$
Issue-Key: iot-jobs-data|UpdateJobExecution|2xx->2xx|$
Issue-Commit: dd79a542b09a035cf711a2d0c52670e9a3e4e70a

# IoT Jobs Data job-execution responses return an empty JSON body, omitting modeled output members

> _Note: this issue was identified against the code base at commit `dd79a542b09a035cf711a2d0c52670e9a3e4e70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartNextPendingJobExecution

## Description
Two causes, one shared fault: the emulator's IoT Jobs Data plane answers job-execution operations with HTTP 200 and a completely empty JSON object ({}), dropping every modeled member of the operation output. The status line is right, so clients see success and then find no payload at all.

Cause 1 of 2: StartNextPendingJobExecution returns 200 with an empty body instead of the modeled output.
AWS: Smithy: StartNextPendingJobExecution -> StartNextPendingJobExecutionResponse with member `execution` (target JobExecution); http PUT /things/{thingName}/jobs/$next, success code 200. The recorded response for this step is a 200 whose body carries the `execution` member. The modeled negative outcomes for an unusable thing are separate errors (ResourceNotFoundException 404, InvalidRequestException 400), not a 200 with no members, so an empty body is not a modeled way to express "nothing to return" here.
Emulator: replay reports status 200 as expected but `$: empty_body (expected {'execution': None}, got {})` — the response document is `{}` and the `execution` member is absent.
Why: a client calling StartNextPendingJobExecution gets a successful call with no JobExecution, so there is no jobId, status, job document or version to act on. SDKs surface this as a success with an unset `execution`, which device agents read as "no pending job" and therefore never pick up or progress the job they just started — a silent no-op rather than an error they could retry or branch on.
Fix: the StartNextPendingJobExecution handler/serializer for this service — the output member is never populated onto (or never written out of) the response shape. Decide between populating `execution` from state and returning the modeled error, rather than emitting a bare 200 {}.
Traces: iotjobsdata-absent-thing-permissive-queue-reads#task#3.

Cause 2 of 2: UpdateJobExecution returns 200 with an empty body, dropping executionState and jobDocument.
AWS: Smithy: UpdateJobExecution -> UpdateJobExecutionResponse with members `executionState` (JobExecutionState) and `jobDocument` (JobDocument, @length 0..32768); http POST /things/{thingName}/jobs/{jobId}, success code 200. Input carries `includeJobExecutionState` and `includeJobDocument` booleans that select exactly those two output members, so when they are requested the response is expected to carry them. The recorded response for this step is a 200 whose body carries both members.
Emulator: replay reports `$: empty_body (expected {'executionState': None, 'jobDocument': None}, got {})` — again a bare `{}` with both modeled members absent.
Why: a device agent updates its execution and asks for the resulting state and/or document in the same round trip. Getting an empty body means it cannot read back the post-update status, the version to use as the next `expectedVersion`, or the job document, so optimistic-concurrency update loops and document-driven agents break even though the update itself reported success.
Fix: the UpdateJobExecution handler/serializer for this service — honor `includeJobExecutionState` / `includeJobDocument` and project `executionState` and `jobDocument` into the response shape instead of serializing an empty document.
Traces: iotjobsdata-job-execution-update-and-terminal-refusals#task#6.

Both causes must be closed before submitting. Note: the two other diverging steps on this service (StartCommandExecution, fixture iotjobsdata-command-execution-validation-refusals) are already owned by the open issue 20261022T071545Z-iot-jobs-data-startcommandexecution-accepts-a-nonexistent-command-and-rejection-messages.md and are NOT part of this issue.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 33 steps, 28 passed, 5 mismatched, score 84.8%, state_reset_failures 0).

Cause 1 — fixture iotjobsdata-absent-thing-permissive-queue-reads, step task#3, StartNextPendingJobExecution:
  expected status 200, actual status 200
  detail: $: empty_body (expected {'execution': None}, got {})
  i.e. expected a response document containing the modeled `execution` member; got {}.

Cause 2 — fixture iotjobsdata-job-execution-update-and-terminal-refusals, step task#6, UpdateJobExecution:
  expected status 200, actual status 200
  detail: $: empty_body (expected {'executionState': None, 'jobDocument': None}, got {})
  i.e. expected a response document containing the modeled `executionState` and `jobDocument` members; got {}.

Model references used: StartNextPendingJobExecutionResponse.execution -> JobExecution; UpdateJobExecutionResponse.executionState -> JobExecutionState and .jobDocument -> JobDocument (@length 0..32768), with input flags includeJobExecutionState / includeJobDocument selecting them.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus, trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture iotjobsdata-absent-thing-permissive-queue-reads and inspect step task#3 — PUT /things/{thingName}/jobs/$next (StartNextPendingJobExecution). Observe HTTP 200 with body {} where the recorded response carries the `execution` member.
3. Cause 2: replay fixture iotjobsdata-job-execution-update-and-terminal-refusals and inspect step task#6 — POST /things/{thingName}/jobs/{jobId} (UpdateJobExecution) requesting the execution state and job document. Observe HTTP 200 with body {} where the recorded response carries `executionState` and `jobDocument`.
4. Both steps are reported as mismatches with verdict `mismatch` and detail `empty_body`; re-replay both fixtures after the fix to confirm the modeled output members are serialized.
