Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-lsrno-traj-invoke-reproduces-timeout
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-lsrno-traj-invoke-reproduces-timeout lambda-lvau-traj-key-rename-breaks-handler lambda-onyx-traj-alpha-is-a-stage-not-an-alias-then-replay-the-records lambda-wsbe-traj-invoke-test-event-shapes-then-endpoint-fix lambda-poller-handlers-invoked-directly lambda-sstr-traj-handler-module-mismatch-then-fix
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|Invoke|2xx->5xx|$.bucket
Issue-Key: lambda|Invoke|2xx->5xx|$.bucket_region
Issue-Key: lambda|Invoke|2xx->5xx|$.request_id
Issue-Key: lambda|Invoke|2xx->5xx|$.table
Issue-Key: lambda|Invoke|2xx->5xx|$.body
Issue-Key: lambda|Invoke|2xx->5xx|$.statusCode
Issue-Key: lambda|Invoke|2xx->5xx|$.id
Issue-Key: lambda|Invoke|2xx->5xx|$.status
Issue-Key: lambda|Invoke|2xx->5xx|$.headers
Issue-Key: lambda|Invoke|2xx->5xx|$.trace
Issue-Related: 20261029T233239Z-lambda-synchronous-invoke-data-plane-answers-501-notimplemented.md
Issue-Commit: dd64cbf0b038e7955d699468def5671676941ec2

# Lambda Invoke does not execute the deployment package and answers 501

> _Note: this issue was identified against the code base at commit `dd64cbf0b038e7955d699468def5671676941ec2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
The synchronous Invoke data plane does not run the function's deployment package: every invocation answers HTTP 501 with x-amzn-errortype NotImplemented instead of executing the handler and returning its payload. 6 causes - six distinct invocation contracts the corpus exercises, each of which produces a different AWS response and all of which the emulator answers identically.

AWS (common to all six): Smithy: Invoke returns HTTP 200 for a RequestResponse invocation, with the handler's return value as the response payload and the header x-amz-executed-version naming the version that ran ('$LATEST' for an unqualified invoke). When the handler raises, AWS still answers HTTP 200 but sets the header x-amz-function-error: 'Unhandled' and returns the error envelope (errorType, errorMessage and a stackTrace/trace array) as the payload.
Emulator (common to all six): answers HTTP 501 with x-amzn-errortype: NotImplemented and no payload and no x-amz-executed-version header.
Why (common to all six): nothing that depends on a function actually running can be exercised - a client cannot read a return value, cannot distinguish a handler error from a platform error, and cannot read the executed version. Every scenario that invokes a function to observe its behaviour is blocked.
Fix: the Invoke data-plane handler - it must execute the deployment package and serialize the handler result (or its unhandled-error envelope) rather than returning NotImplemented.

Cause 1 of 6 - a successful invocation returning a JSON object payload.
AWS: HTTP 200, payload carrying the handler's result object, x-amz-executed-version='$LATEST'.
Emulator: 501, no payload, no header.
Traces: lambda-lsrno-traj-invoke-reproduces-timeout#task#8.

Cause 2 of 6 - an invocation whose handler reads an environment variable the caller just changed.
AWS: HTTP 200 with the payload echoing the configured value, x-amz-executed-version='$LATEST'.
Emulator: 501.
Traces: lambda-lvau-traj-key-rename-breaks-handler#task#10.

Cause 3 of 6 - an invocation returning an API-Gateway-style proxy result (statusCode plus body).
AWS: HTTP 200 with the statusCode/body envelope as the payload, x-amz-executed-version='$LATEST'.
Emulator: 501 (7 steps affected).
Traces: lambda-onyx-traj-alpha-is-a-stage-not-an-alias-then-replay-the-records#task#9.

Cause 4 of 6 - an invocation whose handler returns a record id and a terminal status.
AWS: HTTP 200 with that payload, x-amz-executed-version='$LATEST'.
Emulator: 501 (3 steps affected).
Traces: lambda-poller-handlers-invoked-directly#task#8.

Cause 5 of 6 - an invocation returning a proxy result with headers after a handler fix.
AWS: HTTP 200 with the statusCode/headers/body envelope, x-amz-executed-version='$LATEST'.
Emulator: 501 (2 steps affected).
Traces: lambda-sstr-traj-handler-module-mismatch-then-fix#task#11.

Cause 6 of 6 - an invocation whose handler raises, which must still be HTTP 200 with the unhandled-error contract.
AWS: HTTP 200 with x-amz-function-error='Unhandled', x-amz-executed-version='$LATEST', and a payload carrying errorType, errorMessage and the trace array for the raised exception.
Emulator: 501 with x-amzn-errortype NotImplemented - a platform error rather than a function error, and the wrong status class.
Why specifically: a client that keys on the x-amz-function-error header to tell 'your code threw' from 'Lambda could not run your code' always takes the platform-error branch, so a code bug is reported as a service outage.
Traces: lambda-wsbe-traj-invoke-test-event-shapes-then-endpoint-fix#task#4.

Note: the open issues 20261029T233239Z-lambda-synchronous-invoke-data-plane-answers-501-notimplemented.md and 20261103T020000Z-lambda-opensearch-s3-invoke-execution-and-cloudformation-stack-causes.md describe the same data-plane gap but carry none of these six clusters' replay keys, so these fixtures/steps are not re-replayed when either is closed.

## Evidence
All six clusters: status expected 200, got 501; $headers.x-amz-executed-version expected '$LATEST', got absent; the asserted payload members all absent.
Cause 1: lambda-lsrno-traj-invoke-reproduces-timeout#task#8 - $.request_id, $.bucket, $.bucket_region all expected, all absent.
Cause 2: lambda-lvau-traj-key-rename-breaks-handler#task#10 - $.table expected, absent.
Cause 3: lambda-onyx-traj-alpha-is-a-stage-not-an-alias-then-replay-the-records#task#9 - $.statusCode=200 and $.body='{"processed": 0, "skipped": 1}' expected, absent (7 steps affected).
Cause 4: lambda-poller-handlers-invoked-directly#task#8 - $.id and $.status='SUCCEEDED' expected, absent (3 steps affected).
Cause 5: lambda-sstr-traj-handler-module-mismatch-then-fix#task#11 - $.statusCode, $.headers, $.body expected, absent (2 steps affected).
Cause 6: lambda-wsbe-traj-invoke-test-event-shapes-then-endpoint-fix#task#4 - $.errorType='TypeError', $.errorMessage, $.trace and $headers.x-amz-function-error='Unhandled' expected; got 501 NotImplemented with none of them.

## Steps to Reproduce
1. Replay lambda-lsrno-traj-invoke-reproduces-timeout task#8 and lambda-lvau-traj-key-rename-breaks-handler task#10 (successful RequestResponse invocations).
2. Replay lambda-onyx-traj-alpha-is-a-stage-not-an-alias-then-replay-the-records task#9, lambda-poller-handlers-invoked-directly task#8 and lambda-sstr-traj-handler-module-mismatch-then-fix task#11 (payload-returning invocations).
3. Replay lambda-wsbe-traj-invoke-test-event-shapes-then-endpoint-fix task#4 (a handler that raises) and check for HTTP 200 with x-amz-function-error: Unhandled.
