Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda.conc-limits-and-fanout-reserved-concurrency-admission-transitions-rejected
Issue-Key: dynamodb__lambda.conc-limits-and-fanout-reserved-concurrency-admission-transitions-rejected|_until#53|AssertionError|assert time.time() < deadline, (
Issue-Commit: edd8d27229a8bc99e308a4b32d393f13d6f32cb7

# dynamodb__lambda: DeleteFunctionConcurrency does not release the reserved-concurrency admission limit — invokes keep being refused after the reservation is removed

> _Note: this issue was identified against the code base at commit `edd8d27229a8bc99e308a4b32d393f13d6f32cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteFunctionConcurrency

## Description
Cause 1 of 1: removing a function's reserved concurrency never takes effect on the invoke path.
AWS: Smithy: lambda PutFunctionConcurrency sets ReservedConcurrentExecutions and DeleteFunctionConcurrency (DELETE /2017-10-31/functions/{FunctionName}/concurrency, 204 No Content) removes the reservation entirely; afterwards the function is admitted against the account's unreserved concurrency pool rather than its own reservation, so a concurrent wave that was being throttled at the reserved ceiling is admitted in full. While the reservation is in force, over-the-ceiling concurrent invokes are refused with TooManyRequestsException (HTTP 429, `Rate Exceeded.` / ThrottleReason ConcurrentInvocationLimitExceeded); after DeleteFunctionConcurrency those refusals stop.
Emulator: after DeleteFunctionConcurrency the invoke path still enforces the removed ceiling — a wave of 8 concurrent invokes keeps admitting only 4 and refusing the rest for the full 120 s poll: `removing the reservation never took effect on the invoke path within 120s; last wave admitted [0, 1, 3, 4] and refused [2, 5, 6, 7]` (the particular indices are per-run; the requirement is that no invoke is refused once the reservation is gone).
Why: a client that raises or clears a concurrency reservation to relieve throttling sees no change — its functions stay throttled at a limit that no longer exists, and the admission decision cannot be reasoned about from the control plane's reported state.
Fix: the lambda invoke-path concurrency admission controller and the state DeleteFunctionConcurrency/PutFunctionConcurrency write — the admission check must read the current reservation (absent after a delete) rather than a cached or never-cleared ceiling.
Traces: dynamodb__lambda.conc-limits-and-fanout-reserved-concurrency-admission-transitions-rejected. Related open issue that does not carry this check's replay key: 20261106T151148Z-dynamodb-lambda-reserved-concurrency-is-a-global-invocation.md.

## Evidence
Expected: after DeleteFunctionConcurrency, a concurrent wave is admitted in full (no TooManyRequestsException / 429 refusals attributable to the removed reservation). Actual: `AssertionError: removing the reservation never took effect on the invoke path within 120s; last wave admitted [0, 1, 3, 4] and refused [2, 5, 6, 7]` — the pre-delete ceiling is still being applied 120 s later.

## Steps to Reproduce
Create a function, PutFunctionConcurrency(ReservedConcurrentExecutions=N), drive a concurrent wave larger than N and observe the refusals, then DeleteFunctionConcurrency and drive the same wave again. Observe refusals continuing. Re-replay dynamodb__lambda.conc-limits-and-fanout-reserved-concurrency-admission-transitions-rejected.
