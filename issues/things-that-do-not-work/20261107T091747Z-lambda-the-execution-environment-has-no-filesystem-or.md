Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.ephemeral-storage-available lambda.ephemeral-storage-size-is-enforced-not-only-reported lambda.deep-lambda-runtime-truth-ephemeral-storage-update-resizes-tmp-in-next-environment lambda.deep-lambda-runtime-truth-tmp-empty-in-fresh-env-and-survives-warm lambda.log-lambda-report-stdout-stderr-and-logging-lines-framed-in-stream lambda.arm64-function-reports-aarch64-at-runtime lambda.deep-lambda-runtime-truth-layer-files-byte-exact-at-opt-and-opt-write-rejected lambda.function-sdk-reaches-the-emulator lambda.handler-import-failure-raises-rather-than-echoing lambda.environment-identity-warm-reuse-and-config-update-forces-cold lambda.log-lambda-report-init-duration-only-on-cold-report-line lambda.allocation-beyond-memory-size-is-killed-out-of-memory-rejected lambda.init-phase-is-not-charged-against-the-function-timeout lambda.init-past-the-init-limit-is-retried-inside-the-invoke-and-timed-out-rejected lambda.handler-working-until-its-remaining-time-floor-completes-before-the-kill
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.ephemeral-storage-available|_invoke_sync#5|AssertionError|raise AssertionError(
Issue-Key: lambda.ephemeral-storage-size-is-enforced-not-only-reported|_invoke_json#7|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.deep-lambda-runtime-truth-ephemeral-storage-update-resizes-tmp-in-next-environment|_fill#5|AssertionError|assert not resp.get("FunctionError"), (
Issue-Key: lambda.deep-lambda-runtime-truth-tmp-empty-in-fresh-env-and-survives-warm|_invoke#5|AssertionError|assert not resp.get("FunctionError"), (
Issue-Key: lambda.log-lambda-report-stdout-stderr-and-logging-lines-framed-in-stream|_invoke#7|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.arm64-function-reports-aarch64-at-runtime|_body#8|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.deep-lambda-runtime-truth-layer-files-byte-exact-at-opt-and-opt-write-rejected|_invoke#10|AssertionError|assert not resp.get("FunctionError"), (
Issue-Key: lambda.function-sdk-reaches-the-emulator|_invoke#18|AssertionError|raise AssertionError(
Issue-Key: lambda.handler-import-failure-raises-rather-than-echoing|run#15|AssertionError|assert sdk_error is None, (
Issue-Key: lambda.environment-identity-warm-reuse-and-config-update-forces-cold|run#11|AssertionError|assert len(warm_envs) < len(warm), (
Issue-Key: lambda.log-lambda-report-init-duration-only-on-cold-report-line|run#8|AssertionError|assert warm_body["env"] == cold_body["env"], (
Issue-Key: lambda.allocation-beyond-memory-size-is-killed-out-of-memory-rejected|_assert_out_of_memory#8|AssertionError|assert resp.get("FunctionError") == "Unhandled", (
Issue-Key: lambda.init-phase-is-not-charged-against-the-function-timeout|run#7|AssertionError|assert resp.get("FunctionError") is None, (
Issue-Key: lambda.init-past-the-init-limit-is-retried-inside-the-invoke-and-timed-out-rejected|run#11|AssertionError|assert body.get("errorType") == "Sandbox.Timedout", f"errorType: {body!r}"
Issue-Key: lambda.handler-working-until-its-remaining-time-floor-completes-before-the-kill|run#9|AssertionError|assert isinstance(worked, float) and worked >= _TIMEOUT_S - 1.0, (
Issue-Commit: 677b6e35c5226b67b9bfea5e554d250e3c084a9c

# lambda: the execution environment has no filesystem or process surface, a crippled stdlib, a 4-service botocore, is never reused, and enforces neither MemorySize nor the init/timeout split (7 net-new causes)

> _Note: this issue was identified against the code base at commit `677b6e35c5226b67b9bfea5e554d250e3c084a9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Seven net-new root causes in the Lambda execution environment the Invoke path builds. Open issues report the missing AWS SDK for other service scopes (e.g. 20261105T111951Z-lambda-s3-the-lambda-python-runtime-has-no.md, 20261105T083215Z-ec2-lambda-logs-route53resolver-the-lambda-python-runtime-has-a-crippled-stdlib.md); none of them carries the replay keys below and none covers environment reuse, MemorySize enforcement or init accounting.

Cause 1 of 7: ordinary filesystem and fd syscalls fault.
AWS: recorded behavior only; AWS gives every invocation a writable /tmp sized by EphemeralStorage.Size (512–10240 MB) and a normal POSIX process, so os.statvfs('/tmp'), os.write(1, ...) and os.path.getsize() work and the handler returns a payload.
Emulator: Invoke answers 200 with X-Amz-Function-Error: Unhandled and payload errorType OSError, errorMessage "a Lambda function in this emulator has no host filesystem or process to reach" (fixed text; the stackTrace frame names whichever call the handler made).
Why: a client cannot use /tmp at all, and cannot observe that EphemeralStorage is a real limit rather than a reported number; a handler writing to fd 1/2 fails instead of logging.
Fix: the Invoke path's runtime sandbox — a real (or faithfully emulated) /tmp and fd surface.
Traces: lambda.ephemeral-storage-available, lambda.ephemeral-storage-size-is-enforced-not-only-reported, lambda.deep-lambda-runtime-truth-ephemeral-storage-update-resizes-tmp-in-next-environment, lambda.deep-lambda-runtime-truth-tmp-empty-in-fresh-env-and-survives-warm, lambda.log-lambda-report-stdout-stderr-and-logging-lines-framed-in-stream.

Cause 2 of 7: platform/uname is unavailable, so a function cannot report its architecture.
AWS: Architectures is modeled on CreateFunction/UpdateFunctionCode (ArchitecturesList, exactly one of x86_64|arm64); an arm64 function's runtime reports machine 'aarch64'.
Emulator: the same OSError "a Lambda function in this emulator has no host filesystem or process to reach" is raised from platform.machine() / platform.uname(), so Architectures=['arm64'] is a control-plane field with no runtime meaning.
Why: a client validating an arm64 build cannot tell the two architectures apart.
Fix: the runtime sandbox's platform/uname surface, keyed off the function's Architectures.
Traces: lambda.arm64-function-reports-aarch64-at-runtime.

Cause 3 of 7: the runtime's stdlib is missing extension modules.
AWS: recorded behavior only; the managed Python runtimes ship a complete CPython, so `import` of a module that pulls in _signal succeeds.
Emulator: Runtime.ImportModuleError "Unable to import module '<handler module>': No module named '_signal'".
Why: handlers using ordinary stdlib facilities fail, and the error blames the customer's module.
Fix: the runtime sandbox's stdlib/extension-module set.
Traces: lambda.deep-lambda-runtime-truth-layer-files-byte-exact-at-opt-and-opt-write-rejected.

Cause 4 of 7: the bundled botocore knows only four services.
AWS: recorded behavior only; AWS's managed Python runtimes bundle a full boto3/botocore, so boto3.client('sts').get_caller_identity() works from a handler.
Emulator: Unhandled with errorType UnknownServiceError and errorMessage "Unknown service: 'sts'. Valid service names are: sqs, events, eventbridge, dynamodb" — the service list is the emulator's own, and any other client() call fails the invoke.
Why: in-function AWS calls (the common Lambda pattern) only work for four services, and the failure surfaces as a client-side SDK error.
Fix: the runtime sandbox's bundled botocore service data.
Traces: lambda.function-sdk-reaches-the-emulator, lambda.handler-import-failure-raises-rather-than-echoing.

Cause 5 of 7: the execution environment is never reused — every invocation is a cold start.
AWS: recorded behavior only; AWS reuses a warm environment for back-to-back invocations (module-scope state and the environment identity persist), and only emits an Init Duration on the cold REPORT line.
Emulator: six serialised invocations with ReservedConcurrentExecutions=1 each reported a distinct environment id, and two back-to-back invocations reported different ids.
Why: no client can test warm-start behaviour — connection reuse, module-scope caching, /tmp persistence, Init Duration accounting — all of which are central to Lambda performance work.
Fix: execution-environment lifecycle/reuse in the Lambda data plane invoke path.
Traces: lambda.environment-identity-warm-reuse-and-config-update-forces-cold, lambda.log-lambda-report-init-duration-only-on-cold-report-line.

Cause 6 of 7: MemorySize is not enforced at runtime.
AWS: MemorySize is modeled with range 128–32768 MB; AWS kills an invocation that allocates beyond it, and Invoke answers 200 with X-Amz-Function-Error: Unhandled.
Emulator: a handler that holds 400 MB in a 128 MB function returns normally with FunctionError absent, while reporting memory_limit_in_mb='128'.
Why: a client sizing a function cannot observe the limit it is paying for; out-of-memory handling and retry logic are untestable.
Fix: memory accounting/enforcement in the invoke execution path.
Traces: lambda.allocation-beyond-memory-size-is-killed-out-of-memory-rejected.

Cause 7 of 7: init time is charged against the function Timeout.
AWS: Timeout is modeled range 1–5400 s and applies to the handler invocation; AWS gives init its own (longer) budget, so a 5 s init on a Timeout=3 function still completes the cold invoke, and a handler is allowed to work until its remaining-time floor.
Emulator: the cold invoke fails with Unhandled "<request id> Task timed out after 3.00 seconds" (fixed text: "<requestId> Task timed out after <Timeout>.00 seconds"), and in the floor check the handler got only ~1.5 s of a 3 s Timeout.
Why: functions with legitimate initialisation are reported as timing out, and remaining-time budgeting computed from context.get_remaining_time_in_millis() is wrong.
Fix: invoke timeout accounting — separate the init budget from the handler Timeout in the data-plane invoke path.
Traces: lambda.init-phase-is-not-charged-against-the-function-timeout, lambda.init-past-the-init-limit-is-retried-inside-the-invoke-and-timed-out-rejected, lambda.handler-working-until-its-remaining-time-floor-completes-before-the-kill.

## Evidence
Expected vs actual, from the replay:
- ephemeral-storage-available / ephemeral-storage-size-is-enforced-not-only-reported / deep-lambda-runtime-truth-ephemeral-storage-update-resizes-tmp-in-next-environment / deep-lambda-runtime-truth-tmp-empty-in-fresh-env-and-survives-warm: expected a /tmp report; got FunctionError=Unhandled {"errorMessage":"a Lambda function in this emulator has no host filesystem or process to reach","errorType":"OSError"} from os.statvfs('/tmp')
- log-lambda-report-stdout-stderr-and-logging-lines-framed-in-stream: same OSError from os.write(1, ...)
- arm64-function-reports-aarch64-at-runtime: same OSError from platform.machine()
- deep-lambda-runtime-truth-layer-files-byte-exact-at-opt-and-opt-write-rejected: {"errorMessage":"Unable to import module 'index': No module named '_signal'","errorType":"Runtime.ImportModuleError"}
- function-sdk-reaches-the-emulator / handler-import-failure-raises-rather-than-echoing: {"errorMessage":"Unknown service: 'sts'. Valid service names are: sqs, events, eventbridge, dynamodb","errorType":"UnknownServiceError"}
- environment-identity-warm-reuse-and-config-update-forces-cold: AssertionError: 6 serialised invocations under ReservedConcurrentExecutions=1 each reported a new env_id; the execution environment was never reused
- log-lambda-report-init-duration-only-on-cold-report-line: AssertionError: the back-to-back second invocation must reuse the first environment; env ids differed
- allocation-beyond-memory-size-is-killed-out-of-memory-rejected: AssertionError: 400 MB at 128 MB: FunctionError=None, expected 'Unhandled' (handler reported memory_limit_in_mb='128')
- init-phase-is-not-charged-against-the-function-timeout: AssertionError: a 5 s init with Timeout=3 failed the cold invoke: {'errorMessage': '<id> Task timed out after 3.00 seconds'}
- handler-working-until-its-remaining-time-floor-completes-before-the-kill: AssertionError: the handler worked 1.527 s before its floor; expected at least 2.0 s of a 3 s Timeout

## Steps to Reproduce
1. CreateFunction (python3.x, Zip, MemorySize=128, Timeout=3, EphemeralStorage.Size=512) with a handler that (a) calls os.statvfs('/tmp'), (b) calls platform.machine(), (c) calls boto3.client('sts').get_caller_identity(), (d) allocates 400 MB, (e) sleeps 5 s at module scope, (f) reports a per-environment uuid.
2. Invoke it (twice back-to-back for the reuse legs; with ReservedConcurrentExecutions=1 for the serialised leg).
3. Observe 200 + X-Amz-Function-Error: Unhandled with OSError/UnknownServiceError/Runtime.ImportModuleError payloads, a fresh environment id on every invocation, no out-of-memory kill, and a Task-timed-out failure caused by init.
4. Re-replay the check ids in trace_ids.
