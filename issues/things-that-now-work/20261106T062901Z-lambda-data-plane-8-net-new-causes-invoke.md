Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.log-tail-node-json-console-methods-map-to-their-levels lambda.log-tail-node-json-error-argument-adds-error-fields lambda.log-tail-node-json-error-level-drops-lower-console-records lambda.log-tail-node-json-object-argument-stays-structured lambda.log-tail-node-json-unhandled-error-nests-the-error-in-message lambda.log-tail-node-text-console-lines-are-tab-framed lambda.nodejs-context-object-describes-the-invocation lambda.nodejs-process-exit-reports-exit-error-and-reinitializes-rejected lambda.nodejs-v8-heap-exhaustion-aborts-distinct-from-os-level-out-of-memory-rejected lambda.invoke-with-response-stream-managed-streaming lambda.deep-lambda-runtime-truth-stream-error-after-first-chunk-rejected-in-trailer lambda.durable-execution-kms-enforcement lambda.alias-weight-ladder-observed-distribution lambda.reserved-concurrency-is-one-pool-shared-by-latest-version-and-alias-rejected lambda.nodejs-missing-module-rejected-as-import-module-error lambda.nodejs-missing-handler-export-rejected-as-handler-not-found lambda.nodejs-pending-timer-runs-in-next-invocation-when-not-waiting lambda.nodejs-non-serializable-result-rejected
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.log-tail-node-json-console-methods-map-to-their-levels|_invoke_tail#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-tail-node-json-error-argument-adds-error-fields|_invoke_tail#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-tail-node-json-error-level-drops-lower-console-records|_invoke_tail#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-tail-node-json-object-argument-stays-structured|_invoke_tail#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-tail-node-json-unhandled-error-nests-the-error-in-message|_invoke_tail#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-tail-node-text-console-lines-are-tab-framed|_invoke_tail#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.nodejs-context-object-describes-the-invocation|run#8|KeyError|assert c["awsRequestId"] == resp["ResponseMetadata"]["RequestId"], (
Issue-Key: lambda.nodejs-process-exit-reports-exit-error-and-reinitializes-rejected|_invoke_with_id#3|KeyError|resp["ResponseMetadata"]["RequestId"])
Issue-Key: lambda.nodejs-v8-heap-exhaustion-aborts-distinct-from-os-level-out-of-memory-rejected|_invoke#5|KeyError|return resp, body, tail, resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.invoke-with-response-stream-managed-streaming|run#11|AssertionError|assert len(chunk_times) >= 3, f"expected >=3 progressive chunks, got {len(chunk_times)}: {chunk_times}"
Issue-Key: lambda.deep-lambda-runtime-truth-stream-error-after-first-chunk-rejected-in-trailer|run#10|AssertionError|assert [r["seq"] for _, r in ok_records] == [1, 2, 3], f"control records {ok_records!r}"
Issue-Key: lambda.durable-execution-kms-enforcement|_setup#5|AssertionError|raise AssertionError("unreachable: Invoke response did not include a DurableExecutionArn")
Issue-Key: lambda.alias-weight-ladder-observed-distribution|_invoke_once#16|Timeout|return (executed, marker, None)
Issue-Key: lambda.reserved-concurrency-is-one-pool-shared-by-latest-version-and-alias-rejected|_try_invoke#10|Timeout|return "ok", (resp.get("FunctionError"), json.loads(resp["Payload"].read()))
Issue-Key: lambda.nodejs-missing-module-rejected-as-import-module-error|run#8|AssertionError|assert body["errorMessage"] == message, f"{list(files)}: errorMessage {body['errorMessage']!r}"
Issue-Key: lambda.nodejs-missing-handler-export-rejected-as-handler-not-found|run#8|AssertionError|assert body["errorMessage"] == message, f"{handler} ({list(files)}): errorMessage {body['errorMessage']!r}"
Issue-Key: lambda.nodejs-pending-timer-runs-in-next-invocation-when-not-waiting|run#7|AssertionError|assert seen == {"phase": "observe", "fired": [tag]}, (
Issue-Key: lambda.nodejs-non-serializable-result-rejected|run#6|AssertionError|assert body["errorType"] == "Error", f"mode={mode}: reported {body!r}"
Issue-Commit: 5b7d66e77a060af6cd000fd4d7e09192c88c4502

# Lambda data plane: 8 net-new causes (Invoke response carries no request id, response streaming is not progressive, no DurableExecutionArn, two weighted-alias/reserved-concurrency checks hang, node error classification)

> _Note: this issue was identified against the code base at commit `5b7d66e77a060af6cd000fd4d7e09192c88c4502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
8 net-new root causes on the Lambda data plane, all distinct from the nodejs interpreter gaps filed alongside this issue. Open issues 20261105T111951Z-lambda-10-net-new-root-causes-in-the.md and 20261105T223150Z-lambda-2-net-new-causes-x-amzn-trace.md touch neighbouring symptoms; none of the checks below carry their replay keys.

Cause 1 of 8: the Invoke response carries no request id, so ResponseMetadata.RequestId is absent.
AWS: every Lambda API response carries the request id header (x-amzn-RequestId), which is what the SDK surfaces as ResponseMetadata.RequestId; a Node/Python handler's context.awsRequestId equals it. Recorded behavior only for the header name.
Emulator: the SDK's ResponseMetadata holds only HTTPStatusCode/HTTPHeaders/RetryAttempts, so reading ResponseMetadata['RequestId'] raises KeyError: 'RequestId' - the header is never emitted.
Why: any client that logs or correlates invocations by request id, or that matches the handler's awsRequestId to the API response, cannot work; the id only appears inside the error payload, where AWS does not put it.
Fix: the Invoke handler's response serializer (and the shared response-header layer of the lambda service) must set the request-id header on every response.
Traces: lambda.log-tail-node-json-console-methods-map-to-their-levels, lambda.log-tail-node-json-error-argument-adds-error-fields, lambda.log-tail-node-json-error-level-drops-lower-console-records, lambda.log-tail-node-json-object-argument-stays-structured, lambda.log-tail-node-json-unhandled-error-nests-the-error-in-message, lambda.log-tail-node-text-console-lines-are-tab-framed, lambda.nodejs-context-object-describes-the-invocation, lambda.nodejs-process-exit-reports-exit-error-and-reinitializes-rejected, lambda.nodejs-v8-heap-exhaustion-aborts-distinct-from-os-level-out-of-memory-rejected.

Cause 2 of 8: InvokeWithResponseStream delivers one buffered frame instead of progressive chunks.
AWS: Smithy: InvokeWithResponseStream POST /2021-11-15/functions/{FunctionName}/response-streaming-invocations, output member EventStream targets InvokeWithResponseStreamResponseEvent with @streaming - a handler that writes several times before returning produces several PayloadChunk events, the first arriving before the handler finishes.
Emulator: the client observes a single chunk for a handler that wrote three times ("expected >=3 progressive chunks, got 1"), i.e. the body is buffered and flushed once at completion.
Why: streaming clients see no incremental output, so time-to-first-byte behavior and partial consumption are impossible to exercise.
Fix: the InvokeWithResponseStream handler's event-stream writer - flush each handler write as its own PayloadChunk.
Traces: lambda.invoke-with-response-stream-managed-streaming.

Cause 3 of 8: a streaming invocation that fails after the first chunk emits no terminal/control record.
AWS: when a streaming handler errors after bytes are already on the wire the stream ends with an InvokeComplete event carrying ErrorCode/ErrorDetails (the trailer), so the client can tell a truncated body from a complete one.
Emulator: the check finds no control records at all ("control records []").
Why: a client cannot distinguish a complete response from one truncated by a handler error.
Fix: the InvokeWithResponseStream handler's trailer/InvokeComplete emission.
Traces: lambda.deep-lambda-runtime-truth-stream-error-after-first-chunk-rejected-in-trailer.

Cause 4 of 8: Invoke of a durable function returns no DurableExecutionArn.
AWS: Smithy: Invoke output member DurableExecutionArn is bound to header X-Amz-Durable-Execution-Arn (pattern arn:<partition>:lambda:<region>:<account>:function:<name>:<qualifier>/durable-execution/<name>/<id>); a durable invoke returns it.
Emulator: the response has no DurableExecutionArn ("Invoke response did not include a DurableExecutionArn"), so the KMS-enforcement path of durable executions cannot even be reached.
Why: a client that starts a durable execution has no handle to poll or stop it.
Fix: the Invoke handler's durable-execution branch and its response headers.
Traces: lambda.durable-execution-kms-enforcement.

Cause 5 of 8: the weighted-alias traffic-distribution check never completes.
AWS: an alias with RoutingConfig weights splits invocations between the two versions; a sample of invokes reports both versions in the documented proportions.
Emulator: the check does not finish - "Error: Check timed out after 60.0s" - i.e. the fan-out of invokes against a weighted alias never converges.
Why: callers cannot observe weighted routing at all, and a hung call looks like an availability failure.
Fix: the Invoke handler's alias/weight resolution path (and whatever serialises concurrent invokes behind it).
Traces: lambda.alias-weight-ladder-observed-distribution.

Cause 6 of 8: the shared reserved-concurrency pool check never completes.
AWS: ReservedConcurrentExecutions is one pool shared by $LATEST, published versions and aliases, so admission is counted once across all qualifiers; invokes beyond the reservation are throttled with TooManyRequestsException (429).
Emulator: "Error: Check timed out after 60.0s" - the concurrency admission sequence never settles.
Why: a client relying on reserved concurrency as a shared admission limit gets a hang instead of either admission or a 429.
Fix: the reserved-concurrency admission accounting behind the Invoke handler.
Traces: lambda.reserved-concurrency-is-one-pool-shared-by-latest-version-and-alias-rejected.

Cause 7 of 8: module- and handler-resolution failures are reported with the wrong message text.
AWS: a missing module reports errorType Runtime.ImportModuleError with errorMessage "Cannot find module '<module>'" followed by the require stack, and a missing export reports errorType Runtime.HandlerNotFound with the documented "<handler> is undefined or not a function" phrasing; the fixed text is AWS's wording and the variable parts are the module/handler names from the request and package.
Emulator: the texts differ - errorMessage "Error: Cannot find module 'sim2real-not-installed'" (an extra "Error: " prefix and no require stack) and "index.nope is undefined or not a function" delivered without the AWS errorType/trace framing.
Why: clients that match on the documented error text to distinguish a packaging error from a handler-name error mis-classify both.
Fix: the node runtime's loader error mapping in the Invoke handler.
Traces: lambda.nodejs-missing-module-rejected-as-import-module-error, lambda.nodejs-missing-handler-export-rejected-as-handler-not-found.

Cause 8 of 8: warm-environment timer carry-over and JSON serialisation classification.
AWS: a timer still pending when a handler returns runs during the NEXT invocation of the same warm environment, and a result that cannot be JSON-serialised fails the invoke with errorType Error.
Emulator: the next invocation reports fired=[] (the pending timer never ran), and a BigInt result reports errorType TypeError with message "BigInt are forbidden in JSON.stringify" instead of Error.
Why: handlers that rely on the documented event-loop carry-over behave differently, and error-type based handling of serialisation failures breaks.
Fix: the node runtime's execution-environment reuse (event loop retention) and its response serialiser's error classification.
Traces: lambda.nodejs-pending-timer-runs-in-next-invocation-when-not-waiting, lambda.nodejs-non-serializable-result-rejected.

## Evidence
Expected (AWS): Invoke responses carry the x-amzn-RequestId header (SDK ResponseMetadata.RequestId present and equal to the handler's awsRequestId); InvokeWithResponseStream yields one PayloadChunk per handler write plus an InvokeComplete trailer carrying ErrorCode/ErrorDetails when the handler fails mid-stream; a durable Invoke returns X-Amz-Durable-Execution-Arn; weighted aliases and reserved concurrency answer (or throttle with TooManyRequestsException) rather than hang; Runtime.ImportModuleError says "Cannot find module '<m>'" with a require stack and a missing export is Runtime.HandlerNotFound; pending timers run in the next warm invocation; a non-serialisable result reports errorType Error.
Actual (emulator): resp['ResponseMetadata'] has no 'RequestId' (KeyError: 'RequestId'); a three-write streaming handler yields 1 chunk; no control/InvokeComplete records at all; no DurableExecutionArn in the Invoke response; `Error: Check timed out after 60.0s` for the weighted-alias and shared reserved-concurrency sequences; errorMessage "Error: Cannot find module 'sim2real-not-installed'" and "index.nope is undefined or not a function"; fired=[] after the timer invocation; errorType TypeError "BigInt are forbidden in JSON.stringify".

## Steps to Reproduce
1. Replay the behavioral suite and re-run the 18 check ids in trace_ids (service heading `lambda`).
2. Minimal repros: (a) lambda.invoke(FunctionName=f) and read resp['ResponseMetadata']['RequestId']; (b) invoke_with_response_stream on a handler that writes 3 times with awaits in between and count PayloadChunk events; (c) make that handler throw after the first write and look for InvokeComplete/ErrorCode; (d) invoke with a DurableExecutionName and read DurableExecutionArn; (e) publish two versions, point an alias at both with RoutingConfig weights and invoke the alias repeatedly; (f) set ReservedConcurrentExecutions=1 and invoke $LATEST and the alias concurrently; (g) invoke a nodejs function whose index.js requires a missing module, then one whose handler export does not exist; (h) invoke twice a handler that leaves a setTimeout pending; (i) return a BigInt.
