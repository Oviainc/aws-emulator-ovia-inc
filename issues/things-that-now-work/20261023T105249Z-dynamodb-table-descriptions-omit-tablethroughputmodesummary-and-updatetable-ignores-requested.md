Issue-Source: trace-sweep-divergence
Issue-Trace: ddbctl-backup-on-creating-table-and-restore-name-collision
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ddbctl-backup-on-creating-table-and-restore-name-collision ddbctl-delete-table-in-transitional-state-conflict ddbctl-resource-policy-deny-vs-not-found dynamodb-export-lifecycle dynamodb-index-include-projection-echoed-by-describe-table dynamodb-table-control-plane-lifecycle dynamodb-table-lifecycle-core dynamodb-batch-write-and-get-across-items dynamodb-item-crud-roundtrip-with-conditions dynamodb-partiql-insert-select-update-delete dynamodb-point-in-time-export-pipeline dynamodb-point-in-time-recovery-and-export dynamodb-table-billing-mode-update dynamodb-table-tagging-lifecycle dynamodb-transactional-write-and-read-multi-item dynamodb-import-table-pipeline dynamodb-transactional-operations
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.TableThroughputModeSummary
Issue-Key: dynamodb|DeleteTable|2xx->2xx|$.TableDescription.TableThroughputModeSummary
Issue-Key: dynamodb|CreateTable|2xx->2xx|$.TableDescription.TableThroughputModeSummary
Issue-Key: dynamodb|UpdateTable|2xx->2xx|$.TableDescription.TableThroughputModeSummary
Issue-Key: dynamodb|UpdateTable|2xx->2xx|$.TableDescription.WarmThroughput.ReadUnitsPerSecond
Issue-Key: dynamodb|UpdateTable|2xx->2xx|$.TableDescription.WarmThroughput.WriteUnitsPerSecond
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Table descriptions omit TableThroughputModeSummary and UpdateTable ignores requested WarmThroughput

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTable

## Description
The emulator's TableDescription projection is incomplete on every table control-plane path: the on-demand throughput-mode summary is never emitted, and UpdateTable does not take the WarmThroughput the caller asked for. 5 causes, all in the DynamoDB table control plane.

Cause 1 of 5: DescribeTable response omits Table.TableThroughputModeSummary.
AWS: recorded behavior only - the checked-in Smithy model for dynamodb-20120810 has no TableThroughputModeSummary shape (smithy_query shape=TableThroughputModeSummary -> not found; TableDescription members stop at WarmThroughput/MultiRegionConsistency), so the model says nothing about it. The recording shows the live service returning, inside Table, a TableThroughputModeSummary object with TableThroughputMode set to the table's mode (PAY_PER_REQUEST for an on-demand table) plus LastUpdateToPayPerRequestDateTime once the table has been switched to that mode. Status 200 in both.
Emulator: the member is absent from the 200 body (`$.Table.TableThroughputModeSummary: missing ... got None`).
Why: a client that reads the throughput mode from the new summary member (rather than the older BillingModeSummary) sees no mode at all, so billing-mode-conditional logic - "is this table on-demand before I send ProvisionedThroughput?" - takes the wrong branch or NPEs on an absent structure.
Fix: the DescribeTable handler's TableDescription serializer, and the table-state record it reads from (the mode and its last-switch timestamp must be stored when the table is created or updated).
Traces: ddbctl-backup-on-creating-table-and-restore-name-collision#task#5 (cluster of 10 steps, incl. dynamodb-table-lifecycle-core#task#3 and dynamodb-export-lifecycle#task#7).

Cause 2 of 5: DeleteTable response omits TableDescription.TableThroughputModeSummary.
AWS: recorded behavior only - the DeleteTable 200 body carries the same TableThroughputModeSummary (TableThroughputMode PAY_PER_REQUEST, with LastUpdateToPayPerRequestDateTime) inside TableDescription.
Emulator: member absent from the 200 body.
Why: same as cause 1 on the delete path - teardown code that logs or asserts the mode of the table it just deleted reads nothing.
Fix: the DeleteTable handler's TableDescription serializer (same projection as cause 1).
Traces: ddbctl-delete-table-in-transitional-state-conflict#task#5 (cluster of 10, incl. dynamodb-table-tagging-lifecycle#teardown#12).

Cause 3 of 5: CreateTable response omits TableDescription.TableThroughputModeSummary.
AWS: recorded behavior only - the CreateTable 200 body carries TableThroughputModeSummary with TableThroughputMode PAY_PER_REQUEST (and no LastUpdateToPayPerRequestDateTime, since the table was created in that mode rather than switched).
Emulator: member absent.
Why: the mode is invisible from the moment of creation, so clients that cache the CreateTable description never learn it.
Fix: the CreateTable handler's TableDescription serializer; the mode must be recorded in table state at creation.
Traces: dynamodb-import-table-pipeline#task#1 (cluster of 9, incl. dynamodb-transactional-operations#setup#1).

Cause 4 of 5: UpdateTable response omits TableDescription.TableThroughputModeSummary.
AWS: recorded behavior only - after an UpdateTable that puts the table on on-demand, the 200 body carries TableThroughputModeSummary with TableThroughputMode PAY_PER_REQUEST and a LastUpdateToPayPerRequestDateTime timestamp (the recorded timestamp value itself is environment-specific and is only an example).
Emulator: member absent.
Why: the response to the very call that changed the mode does not report the new mode, so a client cannot confirm the switch without an extra DescribeTable - which is also missing it (cause 1).
Fix: the UpdateTable handler's TableDescription serializer plus the state write that stamps the switch time.
Traces: dynamodb-table-control-plane-lifecycle#task#10 (cluster of 3, incl. dynamodb-table-lifecycle-core#task#5) and dynamodb-table-billing-mode-update#task#3.

Cause 5 of 5: UpdateTable does not apply the requested WarmThroughput and reports fixed defaults.
AWS: Smithy: UpdateTable input member WarmThroughput targets WarmThroughput{ReadUnitsPerSecond:long, WriteUnitsPerSecond:long}, and TableDescription.WarmThroughput targets TableWarmThroughputDescription - i.e. the description echoes the warm throughput configured for the base table. The recording shows the values the request asked for coming back in the response (ReadUnitsPerSecond 2, WriteUnitsPerSecond 2).
Emulator: returns the unconfigured defaults instead (ReadUnitsPerSecond 12000, WriteUnitsPerSecond 4000), i.e. the requested WarmThroughput is dropped.
Why: a client that sets warm throughput before a peak event is told its request did not take effect, or - worse - believes a far larger pre-warm is in place than was asked for; the setting is also lost for later DescribeTable reads.
Fix: UpdateTable's handler/state - persist the WarmThroughput from the request onto the table record and echo it from the TableDescription serializer rather than emitting constants.
Traces: dynamodb-table-billing-mode-update#task#3.

## Evidence
Divergence 1 (DescribeTable, status 200->200): $.Table.TableThroughputModeSummary: missing (expected {'LastUpdateToPayPerRequestDateTime': <timestamp>, 'TableThroughputMode': 'PAY_PER_REQUEST'}, got None); 10 affected steps.
Divergence 2 (DeleteTable, 200->200): $.TableDescription.TableThroughputModeSummary: missing (expected {'LastUpdateToPayPerRequestDateTime': <timestamp>, 'TableThroughputMode': 'PAY_PER_REQUEST'}, got None); 10 affected steps.
Divergence 18 (CreateTable, 200->200): $.TableDescription.TableThroughputModeSummary: missing (expected {'TableThroughputMode': 'PAY_PER_REQUEST'}, got None); 9 affected steps.
Divergence 34 (UpdateTable, 200->200): $.TableDescription.TableThroughputModeSummary: missing (expected {'LastUpdateToPayPerRequestDateTime': <shaped-not-asserted>, 'TableThroughputMode': 'PAY_PER_REQUEST'}, got None); 3 affected steps.
Divergence 32 (UpdateTable, 200->200): $.TableDescription.TableThroughputModeSummary missing; $.TableDescription.WarmThroughput.ReadUnitsPerSecond: expected 2, got 12000; $.TableDescription.WarmThroughput.WriteUnitsPerSecond: expected 2, got 4000.
Model evidence: smithy_query(shape=TableThroughputModeSummary) -> not found in services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json, so cause 1-4 are recorded behavior only; smithy_query(shape=WarmThroughput) shows ReadUnitsPerSecond/WriteUnitsPerSecond are caller-supplied longs.

## Steps to Reproduce
1. Replay fixture ddbctl-backup-on-creating-table-and-restore-name-collision, step task#5 (DescribeTable) and compare $.Table.TableThroughputModeSummary.
2. Replay ddbctl-delete-table-in-transitional-state-conflict#task#5 (DeleteTable) and dynamodb-import-table-pipeline#task#1 (CreateTable); both 200 bodies lack TableDescription.TableThroughputModeSummary.
3. Replay dynamodb-table-control-plane-lifecycle#task#10 (UpdateTable to PAY_PER_REQUEST): no TableThroughputModeSummary in the response.
4. Replay dynamodb-table-billing-mode-update#task#3 (UpdateTable with WarmThroughput 2/2): response reports WarmThroughput 12000/4000 and still no TableThroughputModeSummary.
