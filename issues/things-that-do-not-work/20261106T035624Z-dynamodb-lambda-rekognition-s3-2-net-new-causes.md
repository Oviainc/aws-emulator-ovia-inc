Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__rekognition__s3.swallowed-conditional-put-leaves-a-clean-invocation-and-a-stale-row
Issue-Key: dynamodb__lambda__rekognition__s3.swallowed-conditional-put-leaves-a-clean-invocation-and-a-stale-row|run#12|AssertionError|assert ok["StatusCode"] == 200 and "FunctionError" not in ok, (
Issue-Commit: b436db049950dff84efd7ef809509b3a4a0b040d

# dynamodb__lambda__rekognition__s3: 2 net-new causes — a nodejs22.x handler's first Invoke faults FunctionError='Unhandled' where AWS completes cleanly, and the LogType=Tail LogResult carries neither handler stdout nor the error record

> _Note: this issue was identified against the code base at commit `b436db049950dff84efd7ef809509b3a4a0b040d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two distinct net-new root causes under the suite heading `dynamodb__lambda__rekognition__s3`, both grounded in the failing check `swallowed-conditional-put-leaves-a-clean-invocation-and-a-stale-row`. A fixer must close BOTH before submitting. Neither is carried by any open issue: `20261103T031416Z-dynamodb-lambda-rekognition-s3-7-net-new-causes.md` (S3 notification validation/delivery, GetFunction Code.Location, CFN rollback, TransactWriteItems message) and `20261106T031525Z-dynamodb-lambda-rekognition-s3-the-rekognition-s3-trigger-1933c4.md` (the trigger scenario hanging to the harness cap) do not carry this check's replay key, and the already-filed `DetectLabels` scaffold-stub issue is attributed by the report to four OTHER checks, not this one.

Cause 1 of 2: a RequestResponse Invoke of a nodejs22.x ZipFile handler that calls Rekognition DetectLabels and then a conditional DynamoDB PutItem faults instead of completing.
AWS: Smithy: lambda `Invoke` output `FunctionError` is bound to the `X-Amz-Function-Error` header and is present ONLY when the function itself errored; a successful RequestResponse invocation answers HTTP 200 with that header ABSENT and the handler's return value as the payload (here a bare `return;`, which serialises to the JSON literal `null`). The handler in question `require`s `@aws-sdk/client-rekognition` and `@aws-sdk/client-dynamodb` — bundled in the managed nodejs18.x+ runtimes — logs the S3 object key, awaits `detectLabels`, then performs a `PutItem` with `ConditionExpression: 'attribute_not_exists(image_name)'` inside a try/catch that only logs, so on AWS the invocation always returns 200 with no FunctionError, for the first upload and for the re-upload alike.
Emulator: the FIRST invocation (the control, whose condition holds and whose write must succeed) answers StatusCode 200 with `FunctionError` = `'Unhandled'`, so the scenario never gets past its control step and the stale-row behaviour it exists to prove is never exercised.
Hypothesis: the fault is raised inside the execution environment and is not attributable from the response, because the tail carries no error record (that is cause 2). The two candidates are (a) the managed node runtime image ships no AWS SDK v3 clients, so `require('@aws-sdk/client-rekognition')` fails at init — the same shape as the many filed `Runtime.ImportModuleError` findings for the python runtime — or (b) the in-emulator Rekognition `DetectLabels` is an unimplemented scaffold stub (`services/service-com-amazonaws-rekognition-rekognitionservice/src/ops/detect_labels.rs`, already filed separately) answering 501 NotImplemented, so the awaited call throws. Only the emulator's own invoke log can distinguish them, and it is empty.
Why: a client gets a function error for a handler AWS runs to completion. Nothing downstream of the handler can be observed — no DynamoDB row is written, no swallowed-exception behaviour is reachable — so any in-function AWS call from a node runtime is effectively unusable, and the caller is told only 'Unhandled' with no diagnosis.
Fix: the lambda Invoke execution environment for the node runtimes — the runtime image's bundled AWS SDK and the handler's egress to the emulator's service endpoints; and, if the throwing call is DetectLabels, the rekognition DetectLabels handler.
Traces: dynamodb__lambda__rekognition__s3.swallowed-conditional-put-leaves-a-clean-invocation-and-a-stale-row

Cause 2 of 2: `Invoke` with `LogType=Tail` returns a synthetic envelope-only log, omitting the handler's own output, omitting the unhandled-error record, and omitting `Max Memory Used` from the REPORT line.
AWS: Smithy: `InvocationResponse.LogResult` is documented as 'The last 4 KB of the execution log, which is base64-encoded.' That execution log is the real stream the runtime produced: the `START RequestId: <id> Version: <version>` line, then EVERY line the handler wrote to stdout/stderr (this handler's first statement is `console.log(key)`, which runs before any SDK call, and its catch block does `console.log(err)`), then for a fault an `ERROR` record naming the errorType, errorMessage and stack, then `END RequestId: <id>`, then `REPORT RequestId: <id>\tDuration: <d> ms\tBilled Duration: <b> ms\tMemory Size: <m> MB\tMax Memory Used: <u> MB` (with an extra `Init Duration: <i> ms` on a cold start). The record labels (`START`/`END`/`REPORT`/`Duration`/`Billed Duration`/`Memory Size`/`Max Memory Used`, and the `ERROR` record's keys) are the fixed text; the request id, the durations, the memory numbers and the handler's own text are values from the run and the request.
Emulator: the decoded tail is exactly the three envelope lines and nothing else — `START RequestId: <id> Version: $LATEST\nEND RequestId: <id>\nREPORT RequestId: <id>\tDuration: <d> ms\tBilled Duration: <b> ms\tMemory Size: 128 MB\n`. The handler's `console.log` output is absent even though it precedes the failing statement; no `ERROR`/error record appears even though the same response set `FunctionError: 'Unhandled'`; and the REPORT line stops after `Memory Size`, with no `Max Memory Used` field.
Why: the invocation log is the only place AWS surfaces a swallowed in-handler exception, and it is exactly what a client reads to diagnose a fault. An emulator that returns a fabricated envelope makes every function fault undiagnosable from the API, and a client that parses the REPORT line for `Max Memory Used` finds no such field. It also makes a defect like this one self-concealing: the Invoke response says only 'Unhandled'.
Fix: lambda's invoke log capture and `LogResult` assembly — stream the runtime's actual stdout/stderr and the runtime's error record into the per-invocation log, and emit the full REPORT field set.
Traces: dynamodb__lambda__rekognition__s3.swallowed-conditional-put-leaves-a-clean-invocation-and-a-stale-row

Not reached by this replay (so not claimed here): the same check's later assertions that the swallowed `ConditionalCheckFailedException` is retrievable from the `/aws/lambda/<fn>` log group via FilterLogEvents, and that the re-upload leaves the row byte-identical.

## Evidence
Check `dynamodb__lambda__rekognition__s3.swallowed-conditional-put-leaves-a-clean-invocation-and-a-stale-row`, verdict mismatch, at the control step:

AssertionError: the first invocation must succeed; got status 200 and FunctionError 'Unhandled' with log tail 'START RequestId: ede17c32-01e1-437e-a7cb-07c51b093606 Version: $LATEST\nEND RequestId: ede17c32-01e1-437e-a7cb-07c51b093606\nREPORT RequestId: ede17c32-01e1-437e-a7cb-07c51b093606\tDuration: 6.16 ms\tBilled Duration: 7 ms\tMemory Size: 128 MB\n'

(The request id and the duration/billed-duration numbers above are per-run values, not requirements.)

Cause 1 — expected: HTTP 200, `FunctionError` header absent, payload `null`, and the alpha.jpg row present in the table afterwards. Actual: HTTP 200 with `FunctionError: 'Unhandled'`; the check cannot proceed.
Cause 2 — expected (for LogType=Tail): a log containing the handler's `console.log` line, an error record naming errorType/errorMessage/stack for the 'Unhandled' fault, and a REPORT line ending `... Memory Size: <m> MB\tMax Memory Used: <u> MB`. Actual: START + END + REPORT only, no handler output, no error record, and REPORT truncated after `Memory Size: 128 MB`.

## Steps to Reproduce
1. Replay the behavioral suite and re-run the check id in trace_ids.
2. Create an IAM role trusted by lambda.amazonaws.com with logs:*, rekognition:DetectLabels, dynamodb:PutItem, s3:GetObject.
3. Create a DynamoDB table keyed on `image_name` (S) and an S3 bucket; put a JPEG at key `alpha.jpg`.
4. CreateFunction with Runtime=nodejs22.x, Handler=index.handler, a ZipFile whose index.js does: `console.log(key)`, `await new Rekognition({}).detectLabels({Image:{S3Object:{Bucket,Name:key}},MaxLabels:10,MinConfidence:70})`, then `await new DynamoDB({}).putItem({TableName, Item:{image_name:{S:key},labels:{S:names}}, ConditionExpression:'attribute_not_exists(image_name)'})` inside try/catch that only `console.log(err)`, then a bare `return;`. Environment carries BUCKET_NAME and TABLE_NAME.
5. Wait for the function to go Active, then `Invoke(FunctionName=fn, LogType='Tail', Payload={"Records":[{"s3":{"bucket":{"name":bucket},"object":{"key":"alpha.jpg"}}}]})`.
6. Cause 1: read `StatusCode` and whether `FunctionError` is present — AWS returns 200 with no FunctionError; the emulator returns 200 with FunctionError='Unhandled'.
7. Cause 2: base64-decode `LogResult` and look for the handler's `console.log` line, an error record for the fault, and a `Max Memory Used` field on the REPORT line — all three are absent.
