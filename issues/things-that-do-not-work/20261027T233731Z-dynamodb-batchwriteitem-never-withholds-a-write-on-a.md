Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.batch-write-item-throttle-accounting-loses-no-key
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.batch-write-item-throttle-accounting-loses-no-key|run#17|AssertionError|assert len(landed_set) < _TOTAL, (
Issue-Commit: 6f028cab8dcb36872de9487c20527f3d87d76c81

# dynamodb: BatchWriteItem never withholds a write on a 1-WCU PROVISIONED table — no UnprocessedItems, no ProvisionedThroughputExceededException

> _Note: this issue was identified against the code base at commit `6f028cab8dcb36872de9487c20527f3d87d76c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchWriteItem

## Description
One net-new root cause.

Cause 1 of 1: BatchWriteItem applies no write-capacity accounting on a PROVISIONED table, so it never returns a partial success and never refuses.

AWS: a table created with BillingMode=PROVISIONED and ProvisionedThroughput.WriteCapacityUnits=1 admits roughly one ~1 KB write per second. BatchWriteItem (Smithy: BatchWriteItemOutput.UnprocessedItems, a BatchWriteItemRequestMap of @length min 1 max 25, documented as "A map of tables and requests against those tables that were not processed … you can provide this value directly to a subsequent BatchWriteItem operation") answers HTTP 200 for a batch in which only some of the 25 requests could be processed, and places exactly the withheld PutRequest/DeleteRequest entries in UnprocessedItems. When NO item in the batch can be processed for want of provisioned throughput, AWS instead refuses the call with the modeled error ProvisionedThroughputExceededException (Smithy: com.amazonaws.dynamodb#ProvisionedThroughputExceededException, @error client, HTTP 400). Its message is the fixed text "The level of configured provisioned throughput for the table was exceeded. Consider increasing your provisioning level with the UpdateTable API." — the whole sentence pair is fixed text, it carries no values from the request or from state. The invariant a client depends on is that the keys that landed and the keys reported in UnprocessedItems partition the request exactly: no key is both, and no key is neither.

Emulator: against a table whose WriteCapacityUnits is 1, 320 back-to-back BatchWriteItem calls of 25 ~1 KB items each were all accepted whole. A strongly consistent, paginated Scan afterwards found all 8000 requested keys in the table, with 0 keys reported in UnprocessedItems across 0 partial 200 responses and 0 ProvisionedThroughputExceededException refusals — i.e. the configured write budget is enforced not at all and UnprocessedItems is never populated. The positive control in the same check (one 25-item batch against a PAY_PER_REQUEST table) is accepted whole, as AWS does, so the batch shape itself is fine; only the capacity arm is missing.

Why: a client cannot exercise or verify the documented BatchWriteItem drain loop — issue a batch, retry exactly the UnprocessedItems residue with backoff — because the residue is always empty here, so that whole code path is dead against the emulator and a drain bug in it cannot be caught. Capacity planning and provisioned-throughput alarm/retry behavior cannot be reproduced, and a workload that AWS would partially withhold silently succeeds in full, hiding the throttling the application must handle in production.

Fix: the per-table write-capacity accounting in the DynamoDB table state must be consulted by the BatchWriteItem handler (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/batch_write_item.rs) before each contained request, so a partially admitted batch answers 200 with the withheld requests in UnprocessedItems and a wholly refused batch raises ProvisionedThroughputExceededException with AWS's message. This is the same missing per-table limiter named as Cause 2 of the open issue 20261025T024002Z-dynamodb-7-net-new-root-causes-consumedcapacity-breakdown.md, which covers the single-item read and write paths and does not carry this check's replay key or the UnprocessedItems partial-success behavior that is specific to BatchWriteItem.

Traces: dynamodb.batch-write-item-throttle-accounting-loses-no-key.

## Evidence
Expected: on a PROVISIONED table with WriteCapacityUnits=1, 8000 ~1 KB writes issued as 320 BatchWriteItem calls are not all absorbed — some calls return HTTP 200 with a strict subset of their 25 requests in UnprocessedItems, and a call in which nothing can be processed is refused ProvisionedThroughputExceededException (HTTP 400) with message "The level of configured provisioned throughput for the table was exceeded. Consider increasing your provisioning level with the UpdateTable API."

Actual (replay): AssertionError: a 1-unit provisioned write budget must not absorb 8000 ~1 KB writes issued back to back: 8000 of 8000 landed, with 0 keys reported as unprocessed across 0 partial 200s and 0 all-refused exceptions -- i.e. this target enforced no write capacity at all.

Control in the same check: a 25-item BatchWriteItem against a PAY_PER_REQUEST table is accepted whole with no residue and all 25 items readable — matching AWS — so the divergence is isolated to provisioned write-capacity accounting.

## Steps to Reproduce
1. Build and boot the emulator and replay dynamodb.batch-write-item-throttle-accounting-loses-no-key.
2. CreateTable(BillingMode=PAY_PER_REQUEST, KeySchema pk HASH) and BatchWriteItem 25 ~1 KB items -> 200 with no UnprocessedItems (this control passes).
3. CreateTable(BillingMode=PROVISIONED, ProvisionedThroughput={ReadCapacityUnits:2000, WriteCapacityUnits:1}).
4. Issue 320 BatchWriteItem calls back to back, 25 PutRequests each with a ~900-byte payload attribute (8000 distinct pk values).
5. Observe every call returns 200 with UnprocessedItems absent/empty and no ProvisionedThroughputExceededException is ever raised.
6. Scan the table with ConsistentRead=True and page through LastEvaluatedKey: all 8000 keys are present, so the landed set and the reported-unprocessed set do not partition the request — the reported set is empty.
