Issue-Severity: HIGH
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# Repeated requests to a deployed REST stage never complete, so distinct DynamoDB items and their one stream INSERT each are unreachable (check hangs to the 60 s cap)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
One net-new root cause.

Cause 1 of 1: successive requests to a deployed API Gateway REST stage backed by a Lambda that writes DynamoDB produce no response, so neither the per-visit item nor its single stream record can be observed.
AWS: each request to a deployed REST stage is dispatched to the integration independently. N repeat visits therefore land as N distinct items in the table (one per request, keyed by the per-request value the handler derives), and a stream-enabled table emits exactly one INSERT record per new item — no record is emitted for a write that changes nothing. The observable requirement is that the stage ANSWERS each request and the writes and stream records follow one-per-request.
Emulator: the requests never complete. The check reaches no verdict and is terminated by the harness at its 60-second cap, so no item count and no stream record count are ever produced — there is no response to compare with AWS.
Why: the request-per-write and record-per-write invariants are the core of any API-driven, stream-processed design, and neither can be exercised. A caller sees a hang rather than a response, so its own timeout and retry logic fires and it may double-write against real AWS where the emulator wrote nothing.
Fix: the execute-api data plane for a deployed REST stage — the stage request dispatcher and its Lambda integration path must complete every request, and the DynamoDB stream emitter must then produce one INSERT per newly created item.
Traces: apigw-repeat-visits-land-as-distinct-items-and-stream-one-insert-each. Open issues 20261029T235159Z-apigateway-dynamodb-lambda-2-causes-the-malformed-proxy.md and 20261025T081755Z-apigateway-dynamodb-lambda-a-lambda-proxy-integration-answers.md describe deployed-stage non-response for other checks in this service; the replay reports this check as owned by neither, so its replay key is carried here.

## Evidence
apigateway__dynamodb__lambda.apigw-repeat-visits-land-as-distinct-items-and-stream-one-insert-each — verdict: mismatch. Error: Check timed out after 60.0s. No HTTP response was served for the repeated stage requests within the harness cap, so no item count and no stream INSERT count were produced; AWS serves one response, one item and one INSERT record per request.

## Steps to Reproduce
Replay the behavioral suite and re-run apigateway__dynamodb__lambda.apigw-repeat-visits-land-as-distinct-items-and-stream-one-insert-each. Minimal reproduction: create a stream-enabled DynamoDB table; create a Lambda that writes one item per invocation; wire it as an AWS_PROXY integration on a REST API method; create a deployment and stage; issue the same GET to the stage URL several times — expect one response per request, one distinct item per request in the table, and one INSERT record per item on the table's stream; observe that the requests never complete.
