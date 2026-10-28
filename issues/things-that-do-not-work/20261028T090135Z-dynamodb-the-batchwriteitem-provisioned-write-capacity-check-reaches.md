Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.batch-write-item-throttle-accounting-loses-no-key
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.batch-write-item-throttle-accounting-loses-no-key|run#13|Timeout|resp = ddb.batch_write_item(RequestItems={subject: _requests(chunk)})
Issue-Commit: 0ba4a6d0611b8ec8e3b3a33f291da7a865261cbb

# dynamodb: the BatchWriteItem provisioned-write-capacity check reaches no verdict at all — 320 batches of 25 items against a 1-WCU table run out the 60 s cap

> _Note: this issue was identified against the code base at commit `0ba4a6d0611b8ec8e3b3a33f291da7a865261cbb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchWriteItem

## Description
One root cause (changed symptom of an already-open cause — see the note below before opening a second work item).

Cause 1 of 1: BatchWriteItem against a PROVISIONED table with WriteCapacityUnits=1 neither withholds requests nor refuses them, and the resulting unthrottled 8000-write + consistent-Scan path does not terminate inside the harness's 60 s budget, so the check now produces no verdict at all. Failing check: dynamodb.batch-write-item-throttle-accounting-loses-no-key (verdict: mismatch, 'Check timed out after 60.0s').

NOTE FOR THE FIXER / TRIAGE: the open issue 20261027T233731Z-dynamodb-batchwriteitem-never-withholds-a-write-on-a.md carries this same check id in its Issue-Trace-Ids and names the same missing write-capacity accounting. The replay no longer reproduces that issue's recorded key (an AssertionError on `assert len(landed_set) < _TOTAL`) — it does not get that far — so the suite did not mark this failure as owned. If that issue is still open, fix it once and re-replay this check; this entry exists only because the failure is no longer bound to that issue's replay key and because the symptom changed from a fast assertion to a non-terminating run.

AWS: Smithy — BatchWriteItemOutput.UnprocessedItems (BatchWriteItemRequestMap, @length min 1 max 25), documented as "A map of tables and requests against those tables that were not processed ... you can provide this value directly to a subsequent BatchWriteItem operation"; BatchWriteItem's modeled errors include com.amazonaws.dynamodb#ProvisionedThroughputExceededException (@error client, HTTP 400). On a table created with BillingMode=PROVISIONED and ProvisionedThroughput.WriteCapacityUnits=1, roughly one ~1 KB write per second is admitted. A batch in which only some of its 25 requests fit the budget answers HTTP 200 with exactly the withheld PutRequest/DeleteRequest entries in UnprocessedItems; a batch in which NOTHING can be processed is refused with ProvisionedThroughputExceededException, whose message is entirely fixed text carrying no request or state values: "The level of configured provisioned throughput for the table was exceeded. Consider increasing your provisioning level with the UpdateTable API." Either way the caller gets an answer per call quickly, and the keys that landed plus the keys reported in UnprocessedItems partition the request exactly.

Emulator: no per-table write-capacity accounting is consulted by BatchWriteItem, so no call is ever partially withheld and none is ever refused (the same check previously recorded 8000 of 8000 keys landed, 0 reported unprocessed, 0 refusals). With every one of the 320 back-to-back 25-item batches fully admitted and then re-read by a paginated ConsistentRead Scan over 8000 items, the check no longer reaches any of its assertions: it is killed at the 60 s harness cap with no verdict. Hypothesis (neither the model nor the recording states it): the absent limiter turns a run AWS would have throttled into the full 8000-item write-plus-scan workload, which the emulator's BatchWriteItem/Scan path cannot complete inside the budget; a hang in one of those calls would present identically, and the replay output does not distinguish them.

Why: a client cannot exercise the documented drain loop (issue a batch, retry exactly the UnprocessedItems residue with backoff) because the residue is always empty, so that code path is dead against the emulator and a bug in it cannot be caught; provisioned-throughput retry/alarm behavior cannot be reproduced; and because the call path does not even return a verdict within the budget, a caller whose workload AWS would have throttled sees neither the throttle nor a timely response — a batch-write workload simply stops making observable progress instead of being told to slow down.

Fix: the per-table write-capacity limiter in the DynamoDB table state must be consulted by the BatchWriteItem handler (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/batch_write_item.rs) before each contained request, so a partially admitted batch answers 200 with the withheld requests in UnprocessedItems and a wholly refused batch raises ProvisionedThroughputExceededException with AWS's fixed message; the same handler and the Scan path must answer each call promptly rather than letting an unthrottled bulk load run the harness out of time.

Traces: dynamodb.batch-write-item-throttle-accounting-loses-no-key. The check's positive control (a single 25-item BatchWriteItem against a PAY_PER_REQUEST table, accepted whole with no residue) matches AWS, so the batch shape itself is fine and the divergence is isolated to the provisioned-capacity arm.

## Evidence
Expected (AWS): on a table with BillingMode=PROVISIONED and ProvisionedThroughput={ReadCapacityUnits:2000, WriteCapacityUnits:1}, 320 back-to-back BatchWriteItem calls of 25 ~1 KB PutRequests each are not all absorbed — individual calls answer HTTP 200 with a strict subset of their 25 requests in UnprocessedItems, and a call in which nothing can be processed is refused HTTP 400 ProvisionedThroughputExceededException with message 'The level of configured provisioned throughput for the table was exceeded. Consider increasing your provisioning level with the UpdateTable API.' (all fixed text). Each call answers promptly, and landed-keys + reported-unprocessed-keys partition the 8000 requested keys exactly.

Actual (replay): the check is terminated by the harness — 'Error: Check timed out after 60.0s' — before any of its accounting assertions is evaluated: no partial 200, no refusal and no verdict is observed. The previously recorded run of this same check on this emulator showed the underlying state: 8000 of 8000 keys landed, 0 keys reported unprocessed across 0 partial 200s and 0 all-refused exceptions, i.e. no write capacity enforced at all.

Control in the same check (passes before the timeout window): a 25-item BatchWriteItem against a PAY_PER_REQUEST table is accepted whole with no UnprocessedItems and all 25 items readable — matching AWS.

## Steps to Reproduce
1. Build and boot the emulator and replay dynamodb.batch-write-item-throttle-accounting-loses-no-key.
2. CreateTable(BillingMode=PAY_PER_REQUEST, KeySchema=[{pk,HASH}]) and BatchWriteItem of 25 ~1 KB items -> 200 with no UnprocessedItems (control, matches AWS).
3. CreateTable(BillingMode=PROVISIONED, ProvisionedThroughput={ReadCapacityUnits:2000, WriteCapacityUnits:1}).
4. Issue 320 BatchWriteItem calls back to back, 25 PutRequests each with a ~900-byte payload attribute (8000 distinct pk values).
5. Observe that no call returns UnprocessedItems and none is refused ProvisionedThroughputExceededException, and that the sequence plus the following paginated ConsistentRead Scan of the table does not complete within the 60 s check budget — the check is killed with no verdict.
6. With capacity enforcement in place, expect instead: some 200s carrying a strict subset of their requests in UnprocessedItems, wholly-refused batches raising ProvisionedThroughputExceededException with AWS's message, and after UpdateTable to WriteCapacityUnits=500 the withheld keys landing exactly once (landed and reported sets partitioning the 8000 requested keys).
