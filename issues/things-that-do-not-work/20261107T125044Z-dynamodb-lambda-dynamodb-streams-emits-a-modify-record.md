Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda.ddb-stream-writes-that-change-nothing-emit-no-record dynamodb__lambda.conc-limits-and-fanout-reserved-concurrency-admission-transitions-rejected
Issue-Key: dynamodb__lambda.ddb-stream-writes-that-change-nothing-emit-no-record|run#22|AssertionError|assert names == ["INSERT", "MODIFY"], (
Issue-Key: dynamodb__lambda.conc-limits-and-fanout-reserved-concurrency-admission-transitions-rejected|_wave#53|Timeout|return wid, ok, refused, refused_messages, out
Issue-Commit: 49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4

# dynamodb__lambda: DynamoDB Streams emits a MODIFY record for writes that change nothing, and the reserved-concurrency admission check never reaches a verdict (2 causes)

> _Note: this issue was identified against the code base at commit `49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
2 net-new root causes in this service group. Neither failing check is marked ALREADY FILED or OWNED, so no open issue carries their replay keys. Open issues with neighbouring symptoms that do NOT carry these keys: 20261106T151148Z-dynamodb-lambda-reserved-concurrency-is-a-global-invocation.md and 20261029T180026Z-dynamodb-lambda-dynamodb-stream-esm-record-envelope-images-never-observable.md.

Cause 1 of 2: a write that leaves the item byte-identical still produces a stream record. The emulator emits one MODIFY record per accepted write instead of one record per actual item change. Failing check: dynamodb__lambda.ddb-stream-writes-that-change-nothing-emit-no-record.
AWS: recorded behavior only (the DynamoDB Smithy model describes StreamSpecification/StreamViewType but not record emission). DynamoDB Streams records item-level CHANGES: a PutItem that rewrites an existing item with identical attribute values, and an UpdateItem whose SET assigns an attribute its current value, change nothing and therefore append NO record to the shard. Only the initial insert and the write that actually alters the item appear — for the sequence PutItem(new), PutItem(identical), UpdateItem(no-op), UpdateItem(changing) the shard holds exactly two records with eventName INSERT then MODIFY.
Emulator: the shard held four records, eventName ['INSERT', 'MODIFY', 'MODIFY', 'MODIFY'] — one per accepted write. The two surplus records are the no-op writes and are self-identifying: their NewImage and OldImage are equal (both {'n': {'N': '1'}, 'pk': {...}}), i.e. a record whose before- and after-images are identical was appended. The check fails with AssertionError: "the item's records were ['INSERT', 'MODIFY', 'MODIFY', 'MODIFY']". (The sequence numbers, key value and timestamps in the dump are per-run; the requirement is the record count and the eventName sequence.)
Why: every stream consumer is over-delivered. Idempotent re-writes — the standard way clients make retries safe — now look like real mutations, so change-data-capture pipelines, ESM-driven Lambdas, replication and audit consumers fire side effects for events that did not happen, and any consumer that diffs OldImage against NewImage sees a no-difference MODIFY it has no rule for. Billing/throughput reasoning based on record counts is also wrong.
Fix: the dynamodb crate's stream-append path in the item-write handlers/state (PutItem, UpdateItem and their transactional/batch forms) — compare the post-write item against the pre-write item and append a MODIFY record only when they differ, suppressing the record entirely when the write is a no-op.
Traces: dynamodb__lambda.ddb-stream-writes-that-change-nothing-emit-no-record.

Cause 2 of 2: the reserved-concurrency admission contract produces no verdict — the check is killed by the harness at its 60 s cap. Failing check: dynamodb__lambda.conc-limits-and-fanout-reserved-concurrency-admission-transitions-rejected.
AWS: Smithy (lambda) PutFunctionConcurrency — PUT /2017-10-31/functions/{FunctionName}/concurrency, success 200 — takes a required ReservedConcurrentExecutions with @range min 0 and echoes it back; Invoke models TooManyRequestsException as a 429 client error. Reserved concurrency is a hard admission ceiling on that ONE function: with the reservation set to R, at most R executions of it run at once, and a synchronous Invoke arriving while R slots are held is refused 429 TooManyRequestsException (its message is the fixed throttling text for the function-level reservation; it carries no request-derived values). Raising the reservation to a larger R' admits correspondingly more of the next wave, so a wave of W > R' simultaneous invokes splits into exactly min(W, R') successes and W - min(W, R') refusals.
Emulator: the only detail recorded is "Check timed out after 60.0s" — no assertion is reached. The check fires waves of 8 simultaneous synchronous invokes whose handler holds its slot for 5 s, around PutFunctionConcurrency transitions, and classifies each result as admitted or TooManyRequestsException. Hypothesis: the per-function reservation is not enforced as an admission gate, so no invoke is ever refused and every wave is admitted and serialised/held instead of being split, which exhausts the harness budget before the first transition is graded.
Why: a client cannot observe the only effect reserved concurrency has. An over-subscribed function is never throttled, so back-pressure, retry and dead-letter logic built on TooManyRequestsException is never exercised, and a reservation that is far too small ships looking identical to a correct one.
Fix: the lambda crate's invocation admission path (count in-flight executions per function against the stored ReservedConcurrentExecutions and refuse the excess with 429 TooManyRequestsException before dispatch) and the PutFunctionConcurrency handler/state that must make a raised or lowered reservation take effect for subsequent invokes.
Traces: dynamodb__lambda.conc-limits-and-fanout-reserved-concurrency-admission-transitions-rejected.

## Evidence
Cause 1 — Expected (AWS): on a table with StreamViewType NEW_AND_OLD_IMAGES, the sequence PutItem{pk, n=1} (new item), PutItem{pk, n=1} (identical), UpdateItem SET n = :same (:same = 1), UpdateItem SET n = :two appends exactly two records for that key, eventName ['INSERT', 'MODIFY']; the no-op writes append nothing.
Actual (emulator): four records, eventName ['INSERT', 'MODIFY', 'MODIFY', 'MODIFY']. AssertionError: "the item's records were ['INSERT', 'MODIFY', 'MODIFY', 'MODIFY']". The 2nd and 3rd records carry NewImage == OldImage == {'n': {'N': '1'}, 'pk': {'S': <key>}} — a before/after-identical MODIFY — while the 4th correctly shows OldImage n=1 / NewImage n=2.

Cause 2 — Expected (AWS): PutFunctionConcurrency(ReservedConcurrentExecutions=R) returns 200 echoing R; a wave of 8 simultaneous synchronous Invokes of that function, each holding its slot ~5 s, yields exactly min(8, R) successful responses and 8 - min(8, R) refusals with 429 TooManyRequestsException; after PutFunctionConcurrency raises the reservation to 3, the next wave yields 3 successes and 5 refusals. A verdict is reached per wave.
Actual (emulator): "Check timed out after 60.0s" — no wave is graded; no TooManyRequestsException refusal is observed.

## Steps to Reproduce
Cause 1:
1. CreateTable with StreamSpecification {StreamEnabled: true, StreamViewType: NEW_AND_OLD_IMAGES}.
2. PutItem {pk: <key>, n: 1}.
3. PutItem the same item again, byte-identical.
4. UpdateItem on <key> with UpdateExpression 'SET #n = :same' where :same is the current value 1.
5. UpdateItem on <key> with 'SET #n = :two' where :two = 2.
6. Read the shard (GetShardIterator TRIM_HORIZON + GetRecords, or via a stream ESM) and list eventName per record for <key>.
On AWS: ['INSERT', 'MODIFY'] — two records.
On the emulator: ['INSERT', 'MODIFY', 'MODIFY', 'MODIFY'] — the steps 3 and 4 no-ops each appended a MODIFY whose OldImage equals its NewImage.

Cause 2:
1. CreateFunction whose handler sleeps for a caller-supplied hold (5 s) and then writes a ledger item to DynamoDB.
2. PutFunctionConcurrency(FunctionName=<fn>, ReservedConcurrentExecutions=1).
3. Fire 8 simultaneous synchronous Invokes with hold=5 and classify each outcome.
4. PutFunctionConcurrency(FunctionName=<fn>, ReservedConcurrentExecutions=3) and fire another wave of 8.
On AWS: step 3 yields 1 success and 7 refusals with 429 TooManyRequestsException; step 4 yields 3 successes and 5 such refusals.
On the emulator: no invoke is refused, the waves never split, and the check is killed at the harness's 60 s cap with no verdict.
