Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.concurrent-global-table-replica-update-consistency dynamodb.conditional-check-failed-requests-metric-counts-only-the-failures dynamodb.ddb-wf-warm-throughput-defaults-applied dynamodb.ddbctl-table-refuses-changes-while-creating-and-deleting dynamodb.execute-transaction-exists-condition-statement-commits-or-cancels dynamodb.provisioned-write-throttle-reason-and-metric-agreement-rejected dynamodb.restore-to-point-in-time-excludes-writes-after-restore-time
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.concurrent-global-table-replica-update-consistency|run#19|AssertionError|assert (code, message) == expected, f"losing racer with an unrelated refusal: {o}"
Issue-Key: dynamodb.conditional-check-failed-requests-metric-counts-only-the-failures|_poll_until#14|TimeoutError|raise TimeoutError(f'{description}: the run budget ended first; not graded')
Issue-Key: dynamodb.ddb-wf-warm-throughput-defaults-applied|run#10|AssertionError|assert gsi is not None, (
Issue-Key: dynamodb.ddbctl-table-refuses-changes-while-creating-and-deleting|run#9|AssertionError|assert not pending_creating and not pending_deleting, (
Issue-Key: dynamodb|ExecuteTransaction|ValidationException
Issue-Key: dynamodb.provisioned-write-throttle-reason-and-metric-agreement-rejected|run#63|AssertionError|assert seen[name] == expected, (
Issue-Key: dynamodb.restore-to-point-in-time-excludes-writes-after-restore-time|run#25|AssertionError|assert item_b is None, (
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# dynamodb: 7 net-new causes — control-plane states are instantaneous or stuck, PITR restores include post-restore-time writes, PartiQL EXISTS conditions are rejected, and throttle/CCF metrics never publish

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RestoreTableToPointInTime

## Description
Seven distinct causes in table state transitions, point-in-time restore, PartiQL and the metric plane. Each lists its own failing check ids.

Cause 1 of 7: concurrent replica UpdateTable calls are refused with the wrong in-use refusal.
AWS: when two UpdateTable calls race on the same table's replication group, the loser is refused in the way AWS refuses a concurrent replica update, carrying the replica-update context.
Emulator: the losing racer is refused with an unrelated message — `ResourceInUseException: Attempt to change a resource which is still in use: Table is being updated: <table>` — the generic table-busy refusal, where `<table>` is the request's table name and the rest is the emulator's own fixed text.
Why: a client that distinguishes a retryable replica-update race from a table-wide busy state cannot; retry logic keyed on the refusal misbehaves.
Fix: UpdateTable's replica-update concurrency control and its refusal message.
Traces: concurrent-global-table-replica-update-consistency.

Cause 2 of 7: AWS/DynamoDB ConditionalCheckFailedRequests is never published.
AWS: every conditional write that is refused increments `ConditionalCheckFailedRequests` for the table (dimension set TableName); the Sum over the window equals the number of refusals the client observed.
Emulator: the metric never publishes a Sum covering the 12 refusals the check issued — the poll ran until the run budget ended with nothing recorded.
Why: the CCF metric is the standard signal for optimistic-concurrency contention; dashboards and alarms on it read zero while contention is occurring.
Fix: AWS/DynamoDB metric emission from the conditional-write refusal path.
Traces: conditional-check-failed-requests-metric-counts-only-the-failures.

Cause 3 of 7: a global secondary index never reaches ACTIVE.
AWS: a GSI created with default warm throughput becomes `ACTIVE` promptly, and `DescribeTable` reports `IndexStatus: ACTIVE` with the applied WarmThroughput defaults.
Emulator: the index never reaches ACTIVE within the check's window, so the defaults can never be observed as applied.
Why: a client waits for ACTIVE before querying the index; it waits forever.
Fix: the index-state machine in CreateTable/UpdateTable (the GSI must settle to ACTIVE).
Traces: ddb-wf-warm-throughput-defaults-applied.

Cause 4 of 7: a table is never observable in CREATING or DELETING, so the control-plane calls AWS refuses during those states are accepted.
AWS: while `TableStatus` is `CREATING`, `CreateBackup`, `UpdateContinuousBackups`, `UpdateTable` and `DeleteTable` are refused with `ResourceInUseException` (Smithy: all four declare ResourceInUseException at HTTP 400); the same holds while DELETING.
Emulator: across 5 freshly created tables the table left CREATING before the first CreateBackup could be issued — the state is effectively instantaneous — so none of those refusals is ever produced.
Why: clients that serialise control-plane work by catching ResourceInUseException have no backpressure, and concurrent control-plane mutations are silently interleaved.
Fix: the table lifecycle state machine (CREATING/DELETING must be observable and must gate the control-plane operations).
Traces: ddbctl-table-refuses-changes-while-creating-and-deleting.

Cause 5 of 7: a PartiQL transaction statement with an EXISTS condition is rejected as malformed.
AWS: `ExecuteTransaction` accepts a parameterized statement carrying an existence condition and either commits it or cancels the transaction with `TransactionCanceledException` and per-statement CancellationReasons.
Emulator: the call is refused up front with `ValidationException: Validation failed in TransactStatements[0]: Statement wasn't well formed, can't be processed: Expected data manipulation` — the statement is not parsed at all, so neither the commit nor the cancellation path can be reached.
Why: conditional PartiQL transactions are the PartiQL equivalent of a conditional write; they are unusable.
Fix: the PartiQL parser/planner used by ExecuteTransaction (support for existence conditions in parameterized statements).
Traces: execute-transaction-exists-condition-statement-commits-or-cancels.

Cause 6 of 7: throttle metrics are not derived from the counter that produced the refusals.
AWS: when a provisioned table throttles writes, `ThrottledRequests`, `WriteThrottleEvents` and `ConsumedWriteCapacityUnits` are published from the same accounting that produced the client-visible `ProvisionedThroughputExceededException` refusals, so the metric Sum agrees with the count the client observed.
Emulator: the client observed 1699 throttles while `ThrottledRequests`, `WriteThrottleEvents` and `ConsumedWriteCapacityUnits` all read 0 over the probe window.
Why: capacity tuning and throttle alarms are driven entirely by these metrics; they report a table under no pressure while callers are being refused.
Fix: AWS/DynamoDB throttle/capacity metric emission from the same counters as the refusal path.
Traces: provisioned-write-throttle-reason-and-metric-agreement-rejected.

Cause 7 of 7: RestoreTableToPointInTime includes writes made after RestoreDateTime.
AWS: the restored table contains exactly the state as of `RestoreDateTime`; items written after that instant are not present. Smithy: RestoreTableToPointInTime input RestoreDateTime/UseLatestRestorableTime, errors include InvalidRestoreTimeException.
Emulator: the restored table contains an item written after the restore time (`{'pk': 'item-b', 'val': 'after-restore-time'}` in the recording) — the restore copies the current table rather than the point-in-time snapshot.
Why: PITR exists to recover from a bad write. Restoring the bad write defeats the feature and silently gives the operator a false recovery.
Fix: the point-in-time snapshot selection in the RestoreTableToPointInTime handler and the table's change history/state it reads.
Traces: restore-to-point-in-time-excludes-writes-after-restore-time.

## Evidence
Cause 1: losing racer with an unrelated refusal: ('ResourceInUseException', 'Attempt to change a resource which is still in use: Table is being updated: <table>')
Cause 2: TimeoutError: AWS/DynamoDB ConditionalCheckFailedRequests for TableName=<table> to publish a Sum of at least the 12 refusals this check issued: the run budget ended first
Cause 3: AssertionError: WorkflowIndex did not reach ACTIVE within 1s
Cause 4: AssertionError: never observed the refusal of ['CreateBackup', 'UpdateContinuousBackups', 'UpdateTable', 'DeleteTable'] while CREATING and [] while DELETING in 5 fresh tables: ['<table>: left CREATING before CreateBackup', x5]
Cause 5: botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the ExecuteTransaction operation: Validation failed in TransactStatements[0]: Statement wasn't well formed, can't be processed: Expected data manipulation
Cause 6: AssertionError: AWS/DynamoDB ThrottledRequests Sum over the probe window is 0 but the client observed 1699 -- the metric must be derived from the same counter that produced the refusals (all metrics: {'ThrottledRequests': 0, 'WriteThrottleEvents': 0, 'ConsumedWriteCapacityUnits': 0})
Cause 7: AssertionError: item written after the restore time must be excluded from the restored table, got {'val': {'S': 'after-restore-time'}, 'pk': {'S': 'item-b'}}

## Steps to Reproduce
Cause 1: fire two concurrent UpdateTable ReplicaUpdates on one table and read the loser's refusal.
Cause 2: issue N conditional PutItems that fail, then poll GetMetricStatistics for AWS/DynamoDB ConditionalCheckFailedRequests TableName=<table>.
Cause 3: CreateTable with a GSI and default WarmThroughput; poll DescribeTable for IndexStatus ACTIVE.
Cause 4: CreateTable and immediately call CreateBackup/UpdateContinuousBackups/UpdateTable/DeleteTable while TableStatus should still read CREATING.
Cause 5: ExecuteTransaction with a parameterized statement carrying an EXISTS condition.
Cause 6: drive a provisioned table past its write capacity until ProvisionedThroughputExceededException is observed; compare the observed count with ThrottledRequests/WriteThrottleEvents sums.
Cause 7: enable PITR, write item-a, record T, write item-b, RestoreTableToPointInTime with RestoreDateTime=T, then Scan the restored table.
