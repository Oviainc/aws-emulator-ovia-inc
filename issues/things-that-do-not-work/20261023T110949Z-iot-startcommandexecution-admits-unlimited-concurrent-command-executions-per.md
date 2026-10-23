Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.concurrent-command-execution-creation-respects-limit
Issue-Key: iot.concurrent-command-execution-creation-respects-limit|run#19|AssertionError|assert len(failures) > 0, (
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# IoT: StartCommandExecution admits unlimited concurrent command executions per device — the 100-execution device quota is never enforced

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCommandExecution

## Description
The IoT command-execution data path answers StartCommandExecution, but it has no admission control at all: an unbounded number of simultaneously non-terminal command executions can be created against a single target thing. This issue bundles ONE cause.

Cause 1 of 1: a burst of StartCommandExecution calls far beyond the per-device concurrent-execution quota is admitted in full, with no rejection of any kind.
AWS: Smithy (com.amazonaws.iotjobsdataplane#StartCommandExecution, POST /command-executions) declares exactly the two refusals this situation produces — ServiceQuotaExceededException (httpError 402) and ThrottlingException (httpError 429) — alongside ValidationException (400), ResourceNotFoundException (404), ConflictException (409) and InternalServerException (500). AWS IoT Device Management publishes a non-adjustable "command execution concurrency limit" of 100 concurrent (non-terminal) command executions per target device. Recorded behavior: firing 150 StartCommandExecution calls concurrently against one freshly created thing does NOT admit every request — a meaningful fraction comes back refused, and the refusal is one of the two modeled admission codes above (recorded runs show throttling-style admission control rather than always the quota error, so either code is a faithful answer). The requests that ARE admitted are real execution records (status CREATED or IN_PROGRESS, targetArn echoed verbatim) and the device's execution listing reports exactly those and no more. Status and error code are the requirement here; the recorded message text is only an example.
Emulator: every one of the 150 concurrent requests is admitted — zero refusals, so the check's `len(failures) > 0` assertion fails with "every request was silently admitted". The handler services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/start_command_execution.rs validates targetArn, commandArn, parameters and clientToken idempotency and then enqueues unconditionally; nothing counts the target's outstanding non-terminal executions, and neither modeled admission error (ServiceQuotaExceededException 402 / ThrottlingException 429) is ever constructed.
Why: a client that implements the documented back-pressure contract — catch ServiceQuotaExceededException/ThrottlingException, back off, and retry the command once the device drains — never sees that path against the emulator, so the retry/back-off code is dead in test and first executes in production. Worse, a fleet-control client is told that an arbitrary number of commands were accepted for one device, so it reports work as dispatched that real AWS would have refused, and its own concurrency accounting silently diverges from the service's.
Fix: the admission-control step of StartCommandExecution's handler, before the execution record is minted: the per-target count of non-terminal command executions must be read from the crate's command-execution state and a request over the per-device concurrent-execution limit must be refused with one of the operation's modeled admission errors (ServiceQuotaExceededException / 402 or ThrottlingException / 429) instead of being enqueued. The admitted executions must remain individually retrievable and the target's execution listing must report exactly the admitted set.
Traces: iot.concurrent-command-execution-creation-respects-limit.

## Evidence
Suite: simparity-behavioral-aws-v1 — iot 0/10 passed; this check is the only iot failure not already covered by a filed unimplemented-operation issue.

Cause 1 — iot.concurrent-command-execution-creation-respects-limit [mismatch]:
  expected: of 150 concurrent StartCommandExecution calls against one thing (quota: 100 concurrent non-terminal executions per device), at least one is refused, and every refusal code is in {ServiceQuotaExceededException (402), ThrottlingException (429)}
  actual:   AssertionError: expected at least one execution rejected by admission control under a 150-call burst against the 100-execution device quota -- every request was silently admitted

The burst is preceded in the same check by a successful warm-up StartCommandExecution, so the 150 results are measured against a live, authorized thing/command pair; all 150 returned HTTP 200 with a fresh executionId.

Smithy consulted: com.amazonaws.iotjobsdataplane#StartCommandExecution — input targetArn (required, len 0..2048), commandArn (required), parameters, executionTimeoutSeconds (range min 1), clientToken (@idempotencyToken, len 1..64); errors ServiceQuotaExceededException httpError 402, ThrottlingException httpError 429, ValidationException 400, ResourceNotFoundException 404, ConflictException 409, InternalServerException 500.

Emulator code: services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/start_command_execution.rs — request validation (target ARN parsing, command lookup, parameter checks) with no count of the target's outstanding non-terminal executions and no construction of ServiceQuotaExceededException or ThrottlingException anywhere in the handler.

## Steps to Reproduce
1. Boot the emulator and create a target: iot.create_thing(thingName=<fresh name>) and note its thingArn.
2. iot.create_command(commandId=<fresh id>, namespace="AWS-IoT", payload={"content": b"ping", "contentType": "text/plain"}), note commandArn, and poll iot.get_command until it is readable.
3. Build an iot-jobs-data client bound to the host from iot.describe_endpoint(endpointType="iot:Data-ATS").
4. Issue one StartCommandExecution(commandArn, targetArn, executionTimeoutSeconds=3600) as a warm-up and confirm it returns an executionId.
5. Fire 150 further StartCommandExecution calls for the same commandArn/targetArn concurrently (e.g. ThreadPoolExecutor with 50 workers), none reaching a terminal status in the meantime.
6. Observe the outcome split: AWS refuses a meaningful fraction with ServiceQuotaExceededException (HTTP 402) or ThrottlingException (HTTP 429) once the device's 100 concurrent non-terminal executions are outstanding; the emulator returns HTTP 200 with a fresh executionId for all 150.
7. (Follow-up, once the per-target count exists) list the target's command executions and confirm the count equals exactly the number of admitted calls — no phantom or missing admissions.
