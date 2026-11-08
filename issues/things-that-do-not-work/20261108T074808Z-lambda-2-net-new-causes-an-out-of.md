Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.allocation-beyond-memory-size-is-killed-out-of-memory-rejected lambda.published-version-keeps-its-own-memory-limit-rejected-past-it lambda.outbound-dns-resolution-failure-for-unresolvable-hostname-rejected
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.allocation-beyond-memory-size-is-killed-out-of-memory-rejected|_assert_out_of_memory#8|AssertionError|assert body.get("errorType") == "Runtime.OutOfMemory", f"{label}: errorType {body!r}"
Issue-Key: lambda.published-version-keeps-its-own-memory-limit-rejected-past-it|_assert_out_of_memory#12|AssertionError|assert body.get("errorType") == "Runtime.OutOfMemory", f"{label}: errorType {body!r}"
Issue-Key: lambda.outbound-dns-resolution-failure-for-unresolvable-hostname-rejected|run#7|AssertionError|assert control.get("resolved") is True, (
Issue-Commit: 3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e

# lambda: 2 net-new causes — an out-of-memory kill is reported as Runtime.ExitError instead of Runtime.OutOfMemory, and the function sandbox has no network so DNS resolution never works

> _Note: this issue was identified against the code base at commit `3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two net-new root causes in the lambda service, both on the Invoke data plane.

Cause 1 of 2: an invocation killed for exceeding its MemorySize is classified as a generic runtime exit, not as an out-of-memory kill. Failing checks: lambda.allocation-beyond-memory-size-is-killed-out-of-memory-rejected, lambda.published-version-keeps-its-own-memory-limit-rejected-past-it.
AWS: recorded behavior only — the invoke error envelope is not described by a Smithy shape. When a handler allocates past the function's configured MemorySize, AWS kills the sandbox and answers the synchronous Invoke with FunctionError='Unhandled' and a payload whose errorType is the fixed string `Runtime.OutOfMemory` and whose errorMessage is `RequestId: <request id> Error: Runtime exited with error: signal: killed` — the fixed text being `RequestId: `, ` Error: Runtime exited with error: signal: killed`, with the request id taken from the invocation. The same invocation's REPORT line in the log tail carries a tab-delimited field `Error Type: Runtime.OutOfMemory`. This classification is per-version: a published version keeps the MemorySize it was published with, so an allocation that fits $LATEST's raised limit still produces Runtime.OutOfMemory against the version.
Emulator: FunctionError and errorMessage are right, the classification is wrong — errorType is `Runtime.ExitError`. Replay: `AssertionError: 400 MB at 128 MB: errorType {'errorMessage': 'RequestId: <id> Error: Runtime exited with error: signal: killed', 'errorType': 'Runtime.ExitError', ...}` and `AssertionError: version 1, 400 MB: errorType {... 'errorType': 'Runtime.ExitError' ...}`.
Why: a client cannot distinguish an out-of-memory kill from any other abnormal runtime exit. Retry policies, right-sizing automation and alarms that key on Runtime.OutOfMemory never fire, and an operator reading the REPORT line has no Error Type naming the memory limit as the cause.
Fix: the invoke path's runtime-exit classification (the component that turns a killed sandbox into the error envelope and the REPORT line) must emit errorType Runtime.OutOfMemory, and the matching `Error Type: Runtime.OutOfMemory` REPORT field, when the kill is a memory-limit kill — evaluated against the invoked qualifier's MemorySize, not $LATEST's.

Cause 2 of 2: the function sandbox has no outbound network, so even the positive control of DNS resolution fails. Failing check: lambda.outbound-dns-resolution-failure-for-unresolvable-hostname-rejected.
AWS: recorded behavior only. A Lambda sandbox has outbound network and a working resolver: resolving a resolvable public hostname succeeds, while an unresolvable hostname is refused with a name-resolution error (socket.gaierror) — the check needs the resolvable case to work in order to grade the unresolvable one.
Emulator: every resolution attempt is refused by the emulator itself. Replay: `AssertionError: positive control failed: 'example.com' did not resolve inside the sandbox: {'resolved': False, 'error_type': 'gaierror', 'error': 'a Lambda function in this emulator has no host network to reach'}`. The message is the emulator's own announcement from inside the sandbox, so the refusal is the emulator's and not the runner's.
Why: a client cannot model any handler that talks to a hostname — a resolution failure and a reachable host are indistinguishable, so error handling around outbound calls cannot be exercised and a handler's behaviour on a bad hostname is unverifiable.
Fix: the Lambda execution environment's egress/resolver plumbing — a handler must be able to resolve a resolvable hostname, and must get a name-resolution error (not a blanket 'no host network' refusal) only for names that genuinely do not resolve.
Note: 20261029T162908Z-lambda-the-sandbox-egress-check-can-never-be.md is an open issue about the sandbox egress check; it owns lambda.outbound-https-fetches-s3-object-via-virtual-hosted-presigned-url and does not carry this check's replay key.

Traces: the three checks named above.

## Evidence
Cause 1 — Expected (AWS): FunctionError='Unhandled'; payload errorType=`Runtime.OutOfMemory`; payload errorMessage=`RequestId: <rid> Error: Runtime exited with error: signal: killed`; log-tail REPORT line contains the tab field `Error Type: Runtime.OutOfMemory`.
Actual (emulator): errorType=`Runtime.ExitError` (errorMessage matches).
  - lambda.allocation-beyond-memory-size-is-killed-out-of-memory-rejected: `AssertionError: 400 MB at 128 MB: errorType {'errorMessage': 'RequestId: ec8faa94-0a91-429d-bd7f-4f54a00f5241 Error: Runtime exited with error: signal: killed', 'errorType': 'Runtime.ExitError', 'requestId': 'ec8faa94-...', 'stackTrace': []}`
  - lambda.published-version-keeps-its-own-memory-limit-rejected-past-it: `AssertionError: version 1, 400 MB: errorType {... 'errorType': 'Runtime.ExitError' ...}`
(The 400 MB / 128 MB figures and the request ids are this run's values; the requirement is the errorType classification for any allocation past the invoked qualifier's MemorySize.)

Cause 2 — Expected (AWS): a resolvable public hostname resolves inside the sandbox; an unresolvable one raises socket.gaierror.
Actual (emulator): `{'resolved': False, 'error_type': 'gaierror', 'error': 'a Lambda function in this emulator has no host network to reach'}` even for the resolvable control.

## Steps to Reproduce
Cause 1: (a) create a Python function with MemorySize=128 and a handler that allocates ~400 MB; invoke it with LogType='Tail' — expect FunctionError='Unhandled', errorType 'Runtime.OutOfMemory' and a REPORT field 'Error Type: Runtime.OutOfMemory'; observe errorType 'Runtime.ExitError'. (b) publish a version at MemorySize=128, then UpdateFunctionConfiguration to MemorySize=512; invoke the version with the same payload — the version must still be killed Runtime.OutOfMemory. Replay lambda.allocation-beyond-memory-size-is-killed-out-of-memory-rejected and lambda.published-version-keeps-its-own-memory-limit-rejected-past-it.

Cause 2: create a Python function whose handler calls socket.getaddrinfo on a resolvable public hostname and on an unresolvable one; invoke it. The resolvable control fails with 'a Lambda function in this emulator has no host network to reach'. Replay lambda.outbound-dns-resolution-failure-for-unresolvable-hostname-rejected.
