Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs.log-cwl-node-text-multiline-console-log-is-one-event
Issue-Key: lambda__logs.log-cwl-node-text-multiline-console-log-is-one-event|_invoke_tail#7|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Commit: 5b7d66e77a060af6cd000fd4d7e09192c88c4502

# Lambda log tail: a nodejs text-format invoke cannot be correlated because the Invoke response carries no request id

> _Note: this issue was identified against the code base at commit `5b7d66e77a060af6cd000fd4d7e09192c88c4502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the Invoke response omits the request-id header, so the log-tail check cannot name the invocation whose CloudWatch stream it must read.
AWS: every Lambda API response carries the request-id header (x-amzn-RequestId); SDKs expose it as ResponseMetadata.RequestId, and that id is the one the runtime writes into the START/END/REPORT lines of the invocation's log stream, so a client can join the invoke to its log events. A multi-line console.log under the Text log format is delivered as ONE CloudWatch event. Recorded behavior only for the header name.
Emulator: resp['ResponseMetadata'] contains only HTTPStatusCode/HTTPHeaders/RetryAttempts, so reading ResponseMetadata['RequestId'] raises KeyError: 'RequestId'; the check never reaches the log assertion.
Why: a client cannot correlate an invocation with its log events or REPORT line, which is the documented way to diagnose an invocation.
Fix: the Invoke handler's response serializer in the lambda service crate - emit the request-id header on every response (the same defect reported for service `lambda`; this check is listed under `lambda__logs`).
Traces: lambda__logs.log-cwl-node-text-multiline-console-log-is-one-event.

## Evidence
Expected (AWS): lam.invoke(...) returns a response whose ResponseMetadata.RequestId is the invocation's request id, and the function's log stream contains the multi-line console.log as a single event framed with that id.
Actual (emulator): rid = resp["ResponseMetadata"]["RequestId"] raises KeyError: 'RequestId' - no request-id header is returned at all.

## Steps to Reproduce
1. Replay the behavioral suite and re-run lambda__logs.log-cwl-node-text-multiline-console-log-is-one-event.
2. CreateFunction (nodejs22.x, LoggingConfig LogFormat=Text) whose handler does a single console.log with embedded newlines, invoke it with LogType=Tail, and read resp['ResponseMetadata']['RequestId'].
