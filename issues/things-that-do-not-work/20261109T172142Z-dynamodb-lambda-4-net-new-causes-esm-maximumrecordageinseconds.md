Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda.ddb-stream-esm-maximum-record-age-discards-aged-records dynamodb__lambda.ddb-stream-esm-tumbling-window-state-carries-into-a-final-invoke-per-window dynamodb__lambda.function-code-location-serves-the-handler-with-the-old-image-guard dynamodb__lambda.go-handler-ddb-stream-new-and-old-image-delivered-to-the-handler
Issue-Key: dynamodb__lambda.ddb-stream-esm-maximum-record-age-discards-aged-records|run#27|AssertionError|assert not subject_seen, (
Issue-Key: dynamodb__lambda.ddb-stream-esm-tumbling-window-state-carries-into-a-final-invoke-per-window|closed#12|AssertionError|assert unwindowed == [], f"invocation(s) carried no tumbling window: {unwindowed[:2]!r}"
Issue-Key: dynamodb__lambda.function-code-location-serves-the-handler-with-the-old-image-guard|_download#4|PublishedUrlRejected|target, headers = ctx.endpoint_route(url)
Issue-Key: dynamodb__lambda.go-handler-ddb-stream-new-and-old-image-delivered-to-the-handler|_poll#20|TimeoutError|raise TimeoutError(f"{what}: the run budget ended first; not graded")
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# dynamodb__lambda: 4 net-new causes — ESM MaximumRecordAgeInSeconds and tumbling windows are ignored, GetFunction publishes an unreachable Code.Location, and stream records never reach a Go handler

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Four distinct root causes. Each lists its own failing check ids.

Cause 1 of 4: `MaximumRecordAgeInSeconds` is ignored by the DynamoDB-stream poller.
AWS: an event source mapping whose `MaximumRecordAgeInSeconds` is shorter than the age of the oldest available records on the shard does not deliver those records; measured AWS behavior is that such a mapping never invokes the function again on that shard. Smithy (lambda): CreateEventSourceMapping input `MaximumRecordAgeInSeconds` (-1 = infinite).
Emulator: the aged-out mapping delivered the full record set `[0, 1, 2, 3, 99]` within 5s — the same set and the same window as the control mapping created with age -1. The age filter has no effect at all.
Why: record-age bounds are how a consumer sheds a backlog it can no longer usefully process; ignoring them replays stale records and defeats poison-batch/backlog controls.
Fix: the DynamoDB-stream ESM poller's record-age filtering.
Traces: ddb-stream-esm-maximum-record-age-discards-aged-records.

Cause 2 of 4: tumbling-window state is never attached to the invocation event.
AWS: when `TumblingWindowInSeconds` is set, each invocation event carries `window` (start/end), `state` (the previous invocation's returned state), `shardId`, `eventSourceArn` and `isFinalInvokeForWindow`, and the handler's returned `state` is passed to the next invocation in the window, with a final invoke per window. Smithy (lambda): CreateEventSourceMapping input `TumblingWindowInSeconds`, and FunctionResponseTypes/state carried in the event envelope.
Emulator: the invocation arrived with `window: None`, `final: None`, `early: None` and `shardId: None` (only `stateIn`/`stateOut` echoed by the handler itself) — no tumbling-window envelope is produced, so no window ever finalises.
Why: streaming aggregation over tumbling windows cannot be implemented; handlers that read `event['window']` fail or silently aggregate nothing.
Fix: the ESM invocation-envelope builder for tumbling windows (window bounds, shardId, isFinalInvokeForWindow, state propagation).
Traces: ddb-stream-esm-tumbling-window-state-carries-into-a-final-invoke-per-window.

Cause 3 of 4: `GetFunction` publishes a `Code.Location` on a host the emulator does not serve.
AWS: `GetFunction` returns `Code.Location`, a pre-signed URL that the caller can GET to download the exact deployment package; the URL is served by the endpoint AWS published. Smithy (lambda): GetFunction -> FunctionCodeLocation{RepositoryType, Location}.
Emulator: the published Location is `https://awslambda-<region>-tasks.s3.<region>.amazonaws.com/snapshots/<account>/<fn>-<uuid>` — a host that is not one of the emulator's own hosts and that nothing on the emulator answers, so the download is refused before any handler is reached. The emulator announced an address other than the one it is reachable on.
Why: tooling that verifies or re-publishes the deployed artifact (`aws lambda get-function` + download, CI image guards, drift checks) cannot fetch the code at all.
Fix: the GetFunction serialisation of Code.Location, which must publish a URL on the emulator's own host/listener that serves the stored package.
Traces: function-code-location-serves-the-handler-with-the-old-image-guard. The same cause is already filed for a check in another service grouping as 20261109T124205Z-dynamodb-lambda-rekognition-s3-lambda-getfunction-publishes-code.md; that issue does not carry this check's replay key.

Cause 4 of 4: DynamoDB stream records never reach a Go managed-runtime handler.
AWS: a function on the `provided.al2`/Go runtime receives the stream records (INSERT, MODIFY, REMOVE) with their `NewImage`/`OldImage` exactly as a Python handler does.
Emulator: the handler's audit log never shows the INSERT, MODIFY and REMOVE records for the item; the poll ran until the run budget ended with nothing delivered.
Why: any non-Python stream consumer is dead: the ESM reports healthy and the handler is never fed.
Fix: the Lambda runtime/event delivery path for the Go runtime (event envelope marshalling and the runtime API loop).
Traces: go-handler-ddb-stream-new-and-old-image-delivered-to-the-handler.

## Evidence
Cause 1: AssertionError: a DynamoDB-stream ESM whose oldest available records already exceed MaximumRecordAgeInSeconds=60 at mapping-creation time must never invoke the function again on this shard: the aged-out mapping already saw [0, 1, 2, 3, 99] within 5.0s, the same window in which the control (age -1) fully settled.
Cause 2: AssertionError: invocation(s) carried no tumbling window: [{'window': None, 'stateIn': {}, 'stateOut': {'n': '2'}, 'final': None, 'early': None, 'shardId': None, 'records': [...]}]
Cause 3: simparity.published.PublishedUrlRejected: endpoint_url was given https://awslambda-<region>-tasks.s3.<region>.amazonaws.com/snapshots/<account>/<fn>-<uuid> (published by the emulator): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path> (host, account, fn and port are per-run values).
Cause 4: TimeoutError: INSERT, MODIFY and REMOVE records for the item reach the handler's audit log: the run budget ended first.

## Steps to Reproduce
Cause 1: write records to a DynamoDB stream, wait past 60s, then CreateEventSourceMapping with MaximumRecordAgeInSeconds=60 and observe the aged records are delivered.
Cause 2: CreateEventSourceMapping with TumblingWindowInSeconds>0 on a DynamoDB stream; write records; inspect the event for 'window', 'shardId', 'isFinalInvokeForWindow' and state propagation.
Cause 3: CreateFunction with a zip, then lambda:GetFunction and GET the returned Code.Location.
Cause 4: deploy a Go-runtime handler behind a DynamoDB-stream ESM; PutItem/UpdateItem/DeleteItem and look for INSERT/MODIFY/REMOVE in the handler's log.
