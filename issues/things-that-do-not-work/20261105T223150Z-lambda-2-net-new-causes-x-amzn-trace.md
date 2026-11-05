Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.deep-lambda-runtime-truth-reserved-env-vars-track-version-and-log-destination lambda.out-of-memory-invocation-releases-its-reserved-concurrency-slot lambda.reserved-concurrency-zero-on-one-function-leaves-another-unthrottled-rejected lambda.timed-out-invocation-releases-its-reserved-concurrency-slot
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.deep-lambda-runtime-truth-reserved-env-vars-track-version-and-log-destination|_assert_identity#11|AssertionError|assert m, f"{label}: _X_AMZN_TRACE_ID={trace!r} carries no Root=1-<8hex>-<24hex>"
Issue-Key: lambda.out-of-memory-invocation-releases-its-reserved-concurrency-slot|_try_invoke#7|Timeout|return "ok", (resp.get("FunctionError"), json.loads(resp["Payload"].read()))
Issue-Key: lambda.reserved-concurrency-zero-on-one-function-leaves-another-unthrottled-rejected|_try_invoke#11|Timeout|return "ok", (resp.get("FunctionError"), json.loads(resp["Payload"].read()))
Issue-Key: lambda.timed-out-invocation-releases-its-reserved-concurrency-slot|_try_invoke#7|Timeout|return "ok", (resp.get("FunctionError"), json.loads(resp["Payload"].read()))
Issue-Commit: 2730cd672dd608c80462ed6cac2962eb48b48789

# lambda: 2 net-new causes — _X_AMZN_TRACE_ID is empty in the execution environment, and reserved-concurrency admission/release is never observable so three concurrency scenarios never settle

> _Note: this issue was identified against the code base at commit `2730cd672dd608c80462ed6cac2962eb48b48789`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two distinct root causes in the lambda service.

Cause 1 of 2: the reserved environment variable _X_AMZN_TRACE_ID is present but empty in the execution environment, so a handler can never read the tracing header AWS always supplies.
  AWS: Smithy (service-com-amazonaws-lambda-awsgirapiservice): Invoke is POST /2015-03-31/functions/{FunctionName}/invocations and exposes the tracing context to the runtime, not to the caller; the variable itself is a runtime-contract value rather than a modelled member, so this is AWS's documented runtime behavior. AWS sets _X_AMZN_TRACE_ID in the execution environment for every invocation to the X-Ray trace header of that invocation. Its value is a semicolon-separated header whose Root field matches the template Root=1-<8 hex digits>-<24 hex digits> — fixed text "Root=1-", then the invocation's own trace id, which is a per-invocation value and never a constant. It is set for both $LATEST and published versions, and it changes per invocation while the other reserved variables (AWS_LAMBDA_FUNCTION_VERSION, AWS_LAMBDA_FUNCTION_MEMORY_SIZE, AWS_LAMBDA_LOG_GROUP_NAME) track the invoked version and log destination.
  Emulator: the variable is the empty string. The handler reads _X_AMZN_TRACE_ID='' on the first $LATEST invocation and the identity assertion fails: "AssertionError: $LATEST#1: _X_AMZN_TRACE_ID='' carries no Root=1-<8hex>-<24hex>". No trace header is injected, so there is nothing for the runtime or an in-handler SDK to propagate.
  Why: a client relying on AWS's behavior loses the whole tracing chain from inside a function: any handler or instrumentation library that reads _X_AMZN_TRACE_ID to continue a trace, emit a subsegment, or stamp a correlation id gets an empty value and either drops the trace silently or crashes on parsing. Scenarios that assert a function's reserved-variable identity across versions cannot run, so the version- and log-destination tracking that this same check grades is unverified too.
  Fix: the lambda service's execution-environment construction for Invoke — the per-invocation reserved variables must include a well-formed _X_AMZN_TRACE_ID whose Root field matches AWS's shape and changes per invocation, alongside the version/memory/log-group variables.
  Traces: lambda.deep-lambda-runtime-truth-reserved-env-vars-track-version-and-log-destination.

Cause 2 of 2: reserved concurrency is not observable as an admission decision — neither the throttle a full pool must produce nor the release of a slot when an invocation ends — so all three reserved-concurrency scenarios are terminated at their budget without a verdict.
  AWS: Smithy (service-com-amazonaws-lambda-awsgirapiservice): PutFunctionConcurrency is PUT /2017-10-31/functions/{FunctionName}/concurrency with @required ReservedConcurrentExecutions (@range min 0) and errors InvalidParameterValueException (400), ResourceConflictException (409), ResourceNotFoundException (404), TooManyRequestsException (429), ServiceException (500); Invoke lists TooManyRequestsException (429) among its modelled client errors. AWS's documented behavior: a function's reserved concurrency is a per-function pool. Concurrent Invokes beyond the reserved value are refused TooManyRequestsException (HTTP 429) rather than queued; the pool is per function, so setting one function's reserved concurrency to 0 throttles that function alone and leaves a sibling function unthrottled; and a slot is returned to the pool as soon as the invocation ENDS — including when it ends abnormally, whether killed for exceeding its memory size or timed out — so a following invoke is admitted rather than throttled forever.
  Emulator: all three checks are killed by the harness with "Error: Check timed out after 60.0s". No Invoke status, no TooManyRequestsException and no success is produced for the throttled, the sibling or the post-abnormal-termination leg, so neither the admission refusal nor the release can be compared.
  Why: a client relying on AWS's behavior cannot reason about throttling at all. The two failure directions are both invisible: an emulator that admits everything silently lets a load test pass that AWS would throttle 429, and an emulator that never releases a slot after an out-of-memory kill or a timeout wedges a function permanently with no error to explain it. Reserved concurrency is also the mechanism used to fence a function off (reserved=0); that fence being unobservable means it cannot be trusted.
  Fix: the lambda service's concurrency accounting and the Invoke admission path — per-function reserved-concurrency pools must gate concurrent invocations, refuse an over-limit Invoke with TooManyRequestsException (429), keep pools independent per function, and release a slot on every terminal outcome including an out-of-memory kill and a timeout.
  Traces: lambda.out-of-memory-invocation-releases-its-reserved-concurrency-slot; lambda.reserved-concurrency-zero-on-one-function-leaves-another-unthrottled-rejected; lambda.timed-out-invocation-releases-its-reserved-concurrency-slot.

## Evidence
Cause 1 — Expected (AWS): the handler, on both $LATEST and a published version, reads a non-empty _X_AMZN_TRACE_ID whose Root field matches Root=1-<8 hex>-<24 hex>.
Actual (emulator):
  File "check_lambda_deep_lambda_runtime_truth_reserved_env_vars_track_version_and_log_destination.py", line 162, in _assert_identity
    assert m, f"{label}: _X_AMZN_TRACE_ID={trace!r} carries no Root=1-<8hex>-<24hex>"
  AssertionError: $LATEST#1: _X_AMZN_TRACE_ID='' carries no Root=1-<8hex>-<24hex>

Cause 2 — Expected (AWS): with PutFunctionConcurrency(ReservedConcurrentExecutions=N), concurrent Invokes beyond N are refused TooManyRequestsException (HTTP 429); with ReservedConcurrentExecutions=0 on function A, A is throttled and sibling function B still invokes successfully; after an invocation ends by out-of-memory kill or by timeout, the next Invoke is admitted (200) rather than throttled.
Actual (emulator), all three checks:
  lambda.out-of-memory-invocation-releases-its-reserved-concurrency-slot -> 'Error: Check timed out after 60.0s'
  lambda.reserved-concurrency-zero-on-one-function-leaves-another-unthrottled-rejected -> 'Error: Check timed out after 60.0s'
  lambda.timed-out-invocation-releases-its-reserved-concurrency-slot -> 'Error: Check timed out after 60.0s'
No 429, no 200 and no error code were produced on any leg.

## Steps to Reproduce
Cause 1: create a python function whose handler returns os.environ.get('_X_AMZN_TRACE_ID') together with AWS_LAMBDA_FUNCTION_VERSION, AWS_LAMBDA_FUNCTION_MEMORY_SIZE and AWS_LAMBDA_LOG_GROUP_NAME. Invoke $LATEST and inspect the returned _X_AMZN_TRACE_ID: expect a header whose Root field matches Root=1-<8 hex>-<24 hex>; observe ''. Publish a version and invoke it; the same variable is empty there too.

Cause 2: 
1. Create function A (and, for the isolation leg, a second function B) with a handler that sleeps long enough to hold its slot.
2. Call PutFunctionConcurrency on A with ReservedConcurrentExecutions=1 (and =0 for the isolation leg).
3. Fire concurrent Invokes of A beyond the reserved value and inspect the responses: expect the surplus to be refused TooManyRequestsException with HTTP 429.
4. For the isolation leg, with A at 0, Invoke B and expect 200.
5. For the release legs, make the holding invocation end abnormally — allocate past MemorySize so it is killed out-of-memory, or let it exceed Timeout — then Invoke A again and expect 200 because the slot was released. Observe that each scenario never completes instead.
