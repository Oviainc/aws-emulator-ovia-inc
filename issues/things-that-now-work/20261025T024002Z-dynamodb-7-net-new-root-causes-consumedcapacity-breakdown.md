Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.conc-ddb-optimistic-token-fingerprint-covers-return-options-rejected dynamodb.transact-get-items-consumes-twice-a-strongly-consistent-read dynamodb.transact-token-replay-consumed-capacity-is-a-read-not-a-write dynamodb.provisioned-throughput-exceeded-enforced-across-reads dynamodb.provisioned-write-throttle-reason-and-metric-agreement-rejected dynamodb.ddb-ltr-on-demand-table-reports-zero-capacity-and-on-demand-warm-throughput dynamodb.ddb-ltr-provisioned-table-omits-billing-summary-and-round-trips-capacity dynamodb.table-class-round-trips-and-applies-to-indexes dynamodb.resource-policy-explicit-deny-blocks-access-rejected dynamodb.e2e-resource-policy-grant-revoke-access-flow dynamodb.batch-get-item-unprocessed-keys-are-in-storage-and-drain-to-the-exact-set dynamodb.sdk-retry-replays-the-wire-and-only-the-token-stops-the-duplicate dynamodb.hot-partition-key-throttled-while-distributed-keys-at-same-rate-rejected dynamodb.plain-write-racing-a-transaction-rejected dynamodb.conditional-check-failed-requests-metric-counts-only-the-failures dynamodb.resource-policy-explicit-deny-on-getitem-rejected-put-still-allowed
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.conc-ddb-optimistic-token-fingerprint-covers-return-options-rejected|run#10|AssertionError|assert first_cc.get('WriteCapacityUnits', 0) > 0, f'first use was not billed as a write: {first_cc!r}'
Issue-Key: dynamodb.transact-get-items-consumes-twice-a-strongly-consistent-read|run#11|AssertionError|assert tx.get('ReadCapacityUnits') == tx['CapacityUnits'], (
Issue-Key: dynamodb.transact-token-replay-consumed-capacity-is-a-read-not-a-write|run#20|AssertionError|assert len(writers) == 1, (
Issue-Key: dynamodb.provisioned-throughput-exceeded-enforced-across-reads|run#20|AssertionError|assert triggered, (
Issue-Key: dynamodb.provisioned-write-throttle-reason-and-metric-agreement-rejected|run#20|AssertionError|assert len(w1['errors']) > 0, (
Issue-Key: dynamodb.ddb-ltr-on-demand-table-reports-zero-capacity-and-on-demand-warm-throughput|run#7|AssertionError|assert summary.get("LastUpdateToPayPerRequestDateTime") == t["CreationDateTime"], (
Issue-Key: dynamodb.ddb-ltr-provisioned-table-omits-billing-summary-and-round-trips-capacity|run#11|AssertionError|assert "BillingModeSummary" not in t, (
Issue-Key: dynamodb.table-class-round-trips-and-applies-to-indexes|run#12|AssertionError|assert t['TableClassSummary'].get('LastUpdateDateTime'), (
Issue-Key: dynamodb.resource-policy-explicit-deny-blocks-access-rejected|_get_via_role#31|AssertionError|raise AssertionError("expected the role's GetItem to be denied, but it succeeded")
Issue-Key: dynamodb.e2e-resource-policy-grant-revoke-access-flow|run#22|AssertionError|assert pre_grant_code == "AccessDeniedException", (
Issue-Key: dynamodb|PutItem|ValidationException
Issue-Key: dynamodb.sdk-retry-replays-the-wire-and-only-the-token-stops-the-duplicate|run#10|AssertionError|assert len(request_ids) == total and None not in request_ids, (
Issue-Key: dynamodb.hot-partition-key-throttled-while-distributed-keys-at-same-rate-rejected|run#15|Timeout|time.sleep(_QUIESCE_S)
Issue-Key: dynamodb.plain-write-racing-a-transaction-rejected|run#16|Timeout|outcomes = list(pool.map(lambda fn: fn(None), calls))
Issue-Key: dynamodb.conditional-check-failed-requests-metric-counts-only-the-failures|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb.resource-policy-explicit-deny-on-getitem-rejected-put-still-allowed|run#18|Timeout|time.sleep(3)
Issue-Commit: 56c908e9384776713583471da50d0f78dde9fb40

# dynamodb: 7 net-new root causes — ConsumedCapacity breakdown missing, no throughput enforcement, wrong table metadata, data-plane authorization ignored, an emulator-only size cap, and four checks that hang

> _Note: this issue was identified against the code base at commit `56c908e9384776713583471da50d0f78dde9fb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TransactWriteItems

## Description
Seven distinct root causes in DynamoDB's capacity accounting, throughput enforcement, table metadata, authorization and request identity. Each cause below must be closed before this issue is submitted.

=== Cause 1 of 7: ConsumedCapacity omits the ReadCapacityUnits / WriteCapacityUnits breakdown ===
AWS: Smithy: ConsumedCapacity carries TableName, CapacityUnits, ReadCapacityUnits, WriteCapacityUnits, Table, LocalSecondaryIndexes, GlobalSecondaryIndexes and VectorIndexes. ReadCapacityUnits is documented as 'The total number of read capacity units consumed by the operation' and WriteCapacityUnits as the write equivalent. For the operations that consume one kind of capacity, AWS populates the matching arm beside CapacityUnits: a write reports WriteCapacityUnits equal to CapacityUnits, and TransactGetItems reports ReadCapacityUnits beside the total.
Emulator: ConsumedCapacity is returned with TableName and CapacityUnits only — e.g. {'TableName': '...', 'CapacityUnits': 2.0} — with both ReadCapacityUnits and WriteCapacityUnits absent. Under 24 concurrent TransactWriteItems replays of one ClientRequestToken, 0 of 24 responses reported WriteCapacityUnits > 0 (AWS marks exactly one — the call that committed).
Why: a client that bills, budgets or distinguishes read from write cost reads nothing; and the write-units arm is how AWS tells a caller which of several concurrent idempotent replays actually committed, so that signal is gone entirely.
Fix: the ConsumedCapacity builder shared by the data-plane handlers must populate the read/write arms (services/service-com-amazonaws-dynamodb-dynamodb-20120810 capacity accounting, used by put_item.rs / update_item.rs / transact_write_items.rs / transact_get_items.rs).
Traces: dynamodb.conc-ddb-optimistic-token-fingerprint-covers-return-options-rejected, dynamodb.transact-get-items-consumes-twice-a-strongly-consistent-read, dynamodb.transact-token-replay-consumed-capacity-is-a-read-not-a-write.

=== Cause 2 of 7: provisioned throughput is never enforced — no request is ever throttled ===
AWS: on a PROVISIONED table, requests exceeding the provisioned capacity are refused with ProvisionedThroughputExceededException, HTTP 400 (a modeled error of GetItem/Query/Scan/BatchGetItem/TransactGetItems/PutItem and the other data-plane operations).
Emulator: there is no limiter at all. Against a 1-RCU table under load, not one of GetItem/Query/Scan/BatchGetItem/TransactGetItems was ever throttled; against a 1-WCU table, all 1000 writes were accepted at ~1153/s in 0.87 s.
Why: every client retry/backoff path for capacity exhaustion is dead code against the emulator, and capacity-planning behavior cannot be exercised or tested at all.
Fix: a per-table read/write capacity limiter in the table state, consulted by the data-plane handlers, raising ProvisionedThroughputExceededException (services/service-com-amazonaws-dynamodb-dynamodb-20120810 state + data-plane ops).
Traces: dynamodb.provisioned-throughput-exceeded-enforced-across-reads, dynamodb.provisioned-write-throttle-reason-and-metric-agreement-rejected.

=== Cause 3 of 7: DescribeTable's billing-mode and table-class metadata is wrong ===
AWS: Smithy: BillingModeSummary carries BillingMode and LastUpdateToPayPerRequestDateTime ('the time when PAY_PER_REQUEST was last set'), and the shape's own note says a table must have switched to on-demand at least once to return a BillingModeSummary — so a table that has only ever been PROVISIONED OMITS BillingModeSummary entirely. For a table created on-demand, LastUpdateToPayPerRequestDateTime equals the table's creation time. A TableClass change stamps TableDescription's LastUpdateDateTime.
Emulator: a PROVISIONED table returns BillingModeSummary {'BillingMode': 'PROVISIONED'}; a table created on-demand reports a LastUpdateToPayPerRequestDateTime later than (not equal to) its CreationDateTime; and a table-class change leaves LastUpdateDateTime unstamped.
Why: clients that detect capacity mode by the presence of BillingModeSummary mis-classify every provisioned table, and change-detection that polls LastUpdateDateTime never observes the class change.
Fix: the TableDescription serializer and the table state's billing-mode/table-class bookkeeping (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/describe_table.rs plus create_table.rs / update_table.rs state writes).
Traces: dynamodb.ddb-ltr-on-demand-table-reports-zero-capacity-and-on-demand-warm-throughput, dynamodb.ddb-ltr-provisioned-table-omits-billing-summary-and-round-trips-capacity, dynamodb.table-class-round-trips-and-applies-to-indexes.

=== Cause 4 of 7: resource-based-policy and identity denies are not enforced on the data plane ===
AWS: a DynamoDB resource-based policy with an explicit Deny on dynamodb:GetItem for a principal causes that principal's GetItem to be refused with AccessDeniedException, HTTP 400; and before any grant exists, a foreign principal's call is refused with AccessDeniedException. Explicit deny beats any allow.
Emulator: the denied principal's GetItem succeeds, and the pre-grant call succeeds with no error at all (observed 'expected the role's GetItem to be denied, but it succeeded' and 'expected AccessDeniedException before any grant, saw None').
Why: the emulator cannot be used to validate any least-privilege or deny-based data-plane policy — a policy that would block a read in AWS reads the data here.
Fix: the data-plane authorization step must evaluate the table's resource policy and the caller's identity policy before serving the read (services/service-com-amazonaws-dynamodb-dynamodb-20120810 request authorization path). The open issue 20261024T065715Z-dynamodb-iam-sts-dynamodb-data-plane-reads-are-never-authorized-identity.md describes the same area under a different service token and does not carry these two replay keys.
Traces: dynamodb.resource-policy-explicit-deny-blocks-access-rejected, dynamodb.e2e-resource-policy-grant-revoke-access-flow.

=== Cause 5 of 7: an emulator-only per-table size cap refuses writes AWS accepts ===
AWS: DynamoDB has no per-table stored-size limit; a PutItem of a legal item (<=400 KB) succeeds with HTTP 200 however much data the table already holds. The only size-related ValidationException is the 400 KB per-item limit.
Emulator: PutItem is refused with ValidationException, HTTP 400, message 'Table <name> would exceed this emulator\'s maximum stored size of 16777216 bytes' — an emulator-specific 16 MiB cap, surfaced as an AWS error code the request did not earn.
Why: any scenario that stores more than 16 MiB in a table fails with a refusal that does not exist in AWS, and the refusal is indistinguishable (by code) from a genuine client error.
Fix: the storage limit in the table state must not be enforced as a ValidationException on the data plane (services/service-com-amazonaws-dynamodb-dynamodb-20120810 state size guard, reached from put_item.rs/batch_write_item.rs).
Traces: dynamodb.batch-get-item-unprocessed-keys-are-in-storage-and-drain-to-the-exact-set.

=== Cause 6 of 7: retried attempts are not answered with distinct per-attempt request ids ===
AWS: each HTTP attempt is a separate request and is answered with its own x-amzn-RequestId; two deliveries of an SDK-retried call therefore carry two distinct request ids.
Emulator: across 2 deliveries the service returned 1 distinct x-amzn-RequestId value, and that value was None — the header is absent, so no attempt is identifiable.
Why: request ids are the only handle for correlating a retried call with a server-side outcome; without them a client cannot tell a replay from a first attempt, and support/correlation workflows have nothing to quote.
Fix: the response writer must mint and emit x-amzn-RequestId per attempt (the service's response serializer, not an individual handler). The open issue 20261023T114649Z-dynamodb-success-responses-omit-the-request-id-so.md covers the missing id on success responses and does not carry this replay key.
Traces: dynamodb.sdk-retry-replays-the-wire-and-only-the-token-stops-the-duplicate.

=== Cause 7 of 7: four checks never converge and hang to the harness's 60 s cap ===
AWS: each of these polls for a state AWS reaches promptly — a hot partition key being throttled while evenly distributed keys at the same rate are not; a plain write racing a transaction being refused (TransactionConflictException); the ConditionalCheckFailedRequests metric counting only the failed conditional writes; and an explicit deny on GetItem refusing the read while PutItem for the same principal still succeeds.
Emulator: all four run to 'Check timed out after 60.0s' with no response that satisfies the poll. Hypothesis: the condition each one waits for is a side effect of causes 2 and 4 above (no throughput/conflict enforcement, no data-plane authorization) and of DynamoDB metrics never being emitted, so the poll can never terminate.
Why: these behaviors are simply absent, and because the call neither succeeds nor is refused, a client waits indefinitely rather than receiving an error it can handle.
Fix: the per-table throughput/conflict enforcement and data-plane authorization named in causes 2 and 4, plus the CloudWatch metric emission for conditional-check failures; each poll must reach a terminal observation.
Traces: dynamodb.hot-partition-key-throttled-while-distributed-keys-at-same-rate-rejected, dynamodb.plain-write-racing-a-transaction-rejected, dynamodb.conditional-check-failed-requests-metric-counts-only-the-failures, dynamodb.resource-policy-explicit-deny-on-getitem-rejected-put-still-allowed.

## Evidence
Cause 1: AssertionError: first use was not billed as a write: {'TableName': 'sim2real-conc-ddb-opt-...', 'CapacityUnits': 2.0} (no WriteCapacityUnits). AssertionError: TransactGetItems reports the read arm beside the total, got {'TableName': '...', 'CapacityUnits': 2.0}. AssertionError: 0 of 24 concurrent calls reported WriteCapacityUnits > 0, expected exactly 1.
Cause 2: AssertionError: expected ProvisionedThroughputExceededException on at least one read op under load against a 1-RCU table, observed: {'GetItem': set(), 'Query': set(), 'Scan': set(), 'BatchGetItem': set(), 'TransactGetItems': set()}. AssertionError: all 1000 writes were accepted at 1153/s against a 1-WCU table -- there is no provisioned-throughput limiter (accepted 1000, elapsed 0.87s).
Cause 3: AssertionError: a PROVISIONED table must omit BillingModeSummary, got {'BillingMode': 'PROVISIONED'}. AssertionError: a table created on-demand switched at <t+0.5s>, expected its creation time <t>. AssertionError: a class change must stamp LastUpdateDateTime.
Cause 4: AssertionError: expected the role's GetItem to be denied, but it succeeded. AssertionError: expected AccessDeniedException before any grant, saw None.
Cause 5: botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the PutItem operation: Table sim2real-behav-batchget-... would exceed this emulator's maximum stored size of 16777216 bytes. Expected HTTP 200.
Cause 6: AssertionError: standard/1: the service returned 1 distinct x-amzn-RequestId values across 2 deliveries ([None]); each attempt is a separate request and must be answered as one.
Cause 7: Error: Check timed out after 60.0s (four checks).

## Steps to Reproduce
1. Build and boot the emulator and replay every check id in trace_ids.
2. Cause 1: PutItem/UpdateItem and TransactGetItems/TransactWriteItems with ReturnConsumedCapacity=TOTAL and inspect ConsumedCapacity for ReadCapacityUnits/WriteCapacityUnits.
3. Cause 2: create a 1-RCU / 1-WCU PROVISIONED table and drive reads and 1000 writes; observe that nothing is ever refused with ProvisionedThroughputExceededException.
4. Cause 3: DescribeTable on a never-on-demand PROVISIONED table (BillingModeSummary must be absent); DescribeTable on a table created PAY_PER_REQUEST (LastUpdateToPayPerRequestDateTime must equal CreationDateTime); UpdateTable TableClass then DescribeTable (LastUpdateDateTime must be stamped).
5. Cause 4: PutResourcePolicy with an explicit Deny on dynamodb:GetItem for a role, then GetItem as that role; and GetItem as a foreign principal before any grant exists.
6. Cause 5: fill a table past 16 MiB and issue a legal PutItem.
7. Cause 6: force an SDK retry of one call and compare the x-amzn-RequestId of the two deliveries.
8. Cause 7: replay the four hanging checks and observe the 60 s timeout with no terminal observation.
