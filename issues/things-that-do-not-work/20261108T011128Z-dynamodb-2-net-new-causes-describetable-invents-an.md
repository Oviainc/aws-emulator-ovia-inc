Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.ddb-ltr-on-demand-table-reports-zero-capacity-and-on-demand-warm-throughput dynamodb.ddb-wf-describe-omits-undeclared-optional-blocks dynamodb.deep-dynamodb-1-ddbvec-update-table-backfills-preexisting-items
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.ddb-ltr-on-demand-table-reports-zero-capacity-and-on-demand-warm-throughput|run#14|AssertionError|assert "OnDemandThroughput" not in t, (
Issue-Key: dynamodb.ddb-wf-describe-omits-undeclared-optional-blocks|run#8|AssertionError|assert not invented_before, (
Issue-Key: dynamodb|UpdateTable|NotImplemented
Issue-Commit: f4d58d8ba640bd38bd7af13aafdf9baa78485a99

# dynamodb: 2 net-new causes — DescribeTable invents an OnDemandThroughput block with -1/-1 on tables that requested no maximum, and UpdateTable with VectorIndexUpdates Create answers NotImplemented

> _Note: this issue was identified against the code base at commit `f4d58d8ba640bd38bd7af13aafdf9baa78485a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTable

## Description
This issue carries 2 net-new root causes.

Cause 1 of 2: DescribeTable reports an OnDemandThroughput block on tables that never requested an on-demand maximum.
Failing check ids: dynamodb.ddb-ltr-on-demand-table-reports-zero-capacity-and-on-demand-warm-throughput, dynamodb.ddb-wf-describe-omits-undeclared-optional-blocks.
AWS: Smithy: TableDescription carries OnDemandThroughput only as the maximum-request-units setting; OnDemandThroughput.MaxReadRequestUnits/MaxWriteRequestUnits are documented as "To specify a maximum OnDemandThroughput on your table, set the value ... greater than or equal to 1. To remove the maximum ... set the value ... to -1." Recorded behavior: a table created without an OnDemandThroughput request carries no OnDemandThroughput member at all in DescribeTable — the optional block is omitted, not synthesised (a PAY_PER_REQUEST table still reports BillingModeSummary PAY_PER_REQUEST, zero-unit ProvisionedThroughput and the on-demand WarmThroughput baseline).
Emulator: DescribeTable on a freshly created table returns OnDemandThroughput {'MaxReadRequestUnits': -1, 'MaxWriteRequestUnits': -1}, i.e. the sentinel that means "remove the maximum" is serialised as if it were a configured setting.
Why: a client reading back table configuration sees a maximum-throughput setting that was never requested; round-tripping DescribeTable output into CreateTable/UpdateTable then sends -1 values, and drift detection (CloudFormation, Terraform, inventory tooling) reports a difference on every such table.
Fix: the DescribeTable (and CreateTable/UpdateTable) response serializer for TableDescription — omit OnDemandThroughput when no maximum is set instead of emitting -1 defaults.

Cause 2 of 2: UpdateTable with VectorIndexUpdates carrying Create is not implemented, so no vector index can be added to an existing table.
Failing check ids: dynamodb.deep-dynamodb-1-ddbvec-update-table-backfills-preexisting-items.
AWS: Smithy: UpdateTable.VectorIndexUpdates (VectorIndexUpdateList) with errors limited to InternalServerError, InvalidEndpointException, LimitExceededException, ResourceInUseException and ResourceNotFoundException — NotImplemented is not a modelled outcome. Recorded behavior: UpdateTable with a VectorIndexUpdates Create is accepted and the new index is backfilled from the items already in the table, which later Query/SearchVectors reads observe.
Emulator: the call is refused with ClientError (NotImplemented) 'operation UpdateTable with VectorIndexUpdates carrying Create is scaffolded but not implemented in this emulator'.
Why: the error code is not in the operation's modelled error set, so SDK error handling cannot classify it; and no vector index can ever be added to an existing table, making the backfill behavior unreachable.
Fix: the UpdateTable handler's VectorIndexUpdates branch (Create) and the vector-index state it must build, including backfill of pre-existing items.
Note: the open issue 20261027T001720Z-dynamodb-10-net-new-root-causes-searchvectors-ignores.md covers other vector-index defects and does not carry this check's replay key.
Traces: the failing check ids listed per cause, as examples.

## Evidence
Cause 1 — Expected: 'OnDemandThroughput' absent from DescribeTable's Table for a table created without it (PAY_PER_REQUEST table also reporting ProvisionedThroughput 0/0/0 and WarmThroughput 12000/4000). Actual: AssertionError: no maximum was requested, yet describe reports OnDemandThroughput {'MaxReadRequestUnits': -1, 'MaxWriteRequestUnits': -1}; and AssertionError: a freshly created table already carries undeclared block(s) ['OnDemandThroughput'].

Cause 2 — Expected: UpdateTable with VectorIndexUpdates=[{Create: ...}] accepted and the index backfilled from pre-existing items. Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateTable operation: operation UpdateTable with VectorIndexUpdates carrying Create is scaffolded but not implemented in this emulator.

## Steps to Reproduce
1. Cause 1: CreateTable with one HASH key and BillingMode PAY_PER_REQUEST (no OnDemandThroughput), wait for ACTIVE, then DescribeTable — the response carries OnDemandThroughput {MaxReadRequestUnits: -1, MaxWriteRequestUnits: -1}. Repeat with a plain table to see the same undeclared block.
2. Cause 2: create a table, write items, then call UpdateTable with VectorIndexUpdates=[{Create: {...}}] — it is refused with NotImplemented.
3. Or replay the check ids in trace_ids.
