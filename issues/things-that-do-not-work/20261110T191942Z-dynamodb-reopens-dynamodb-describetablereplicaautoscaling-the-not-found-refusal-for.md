Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.replica-autoscaling-not-found
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.replica-autoscaling-not-found|run#6|AssertionError|assert describe_message == expected_describe, f"unexpected message {describe_message!r}"
Issue-Commit: ac0eea33c4f4a66a7179f256fe88fefcc3a441e0

# REOPENS dynamodb DescribeTableReplicaAutoScaling: the not-found refusal for a nonexistent table uses the table-not-found sentence instead of AWS's global-table sentence

> _Note: this issue was identified against the code base at commit `ac0eea33c4f4a66a7179f256fe88fefcc3a441e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTableReplicaAutoScaling

## Description
REOPENS 20261108T011128Z-dynamodb-describetablereplicaautoscaling-stub.md: its fix left the call refused.

Cause 1 of 1: DescribeTableReplicaAutoScaling (and, by the same rule, UpdateTableReplicaAutoScaling) addressed to a table name that does not exist is refused with the generic DynamoDB table-not-found sentence rather than the global-table sentence AWS serves for the replica-auto-scaling pair. Failing check: dynamodb.replica-autoscaling-not-found.

AWS: Smithy: DescribeTableReplicaAutoScaling input `TableName` (target TableArn, required, @length 1..1024); modeled errors `InternalServerError` (HTTP 500) and `ResourceNotFoundException` (HTTP 400). UpdateTableReplicaAutoScaling models the same ResourceNotFoundException (plus LimitExceededException, ResourceInUseException). Both operations are documented against the GLOBAL TABLE ("Describes auto scaling settings across replicas of the global table at once"), so a name the service does not hold is reported as a missing global table. Recorded behavior: HTTP 400, Code `ResourceNotFoundException`, Message `Global table with name: '<TableName>' does not exist.` — fixed text `Global table with name: '` and `' does not exist.`, with `<TableName>` taken verbatim from the request's TableName. The same code and message come back from UpdateTableReplicaAutoScaling for the same missing name.
Emulator: the error class and status are right, the sentence is not. It answers `ResourceNotFoundException` with `Requested resource not found: Table: <TableName> not found` — DynamoDB's generic table-not-found wording, routed through the shared table-resolution refusal instead of the replica-auto-scaling one.
Why: the error message is a requirement. A client (or a fixture/golden-file comparison, or an operator reading a log) that distinguishes "this table does not exist" from "this table is not a global table" cannot: both the nonexistent-table case and the non-replicated-table case have to speak about the global table, and the emulator's sentence names a different resource kind than AWS's. Retry/branching logic keyed on the message text takes the wrong branch.
Fix: the refusal lifting in the DescribeTableReplicaAutoScaling and UpdateTableReplicaAutoScaling handlers — these two operations must not reuse the generic table-resolution not-found message; their own not-found text must be the global-table sentence built from the request's TableName.
Traces: check `dynamodb.replica-autoscaling-not-found` (it asserts the Describe message first, which is where it failed, and then the identical Update message for the same missing name — the Update leg was never reached, but it carries the same requirement). Handler: services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/describe_table_replica_auto_scaling.rs lines 141 and 171-184 (`crate::state::resolve_table_name_in` -> `lift(RequestError::NotFound(message))`, which forwards the store's message unchanged), and the sibling path in services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/update_table_replica_auto_scaling.rs line 165.

## Evidence
Check dynamodb.replica-autoscaling-not-found, DescribeTableReplicaAutoScaling(TableName=<a name no table holds>):
  Expected (real AWS): HTTP 400, Error.Code = ResourceNotFoundException, Error.Message = "Global table with name: '<TableName>' does not exist."
  Actual (emulator): HTTP 400, Error.Code = ResourceNotFoundException (correct), Error.Message = "Requested resource not found: Table: <TableName> not found"
  Replay assertion: "AssertionError: unexpected message 'Requested resource not found: Table: sim2real-behav-no-such-table-... not found'" (the generated table name is this run's value; the requirement is that the message is the global-table sentence with the request's TableName substituted).
  The check then makes the same assertion for UpdateTableReplicaAutoScaling(TableName=<same missing name>, ProvisionedWriteCapacityAutoScalingUpdate={MinimumUnits:1, MaximumUnits:10, ScalingPolicyUpdate:{...}}), expecting the identical code and message; that leg was not reached.

## Steps to Reproduce
1. Pick a table name the emulator holds no table for (e.g. "no-such-table-<random>").
2. Call DescribeTableReplicaAutoScaling(TableName=<that name>).
3. Observe Error.Code/Error.Message: AWS gives ResourceNotFoundException with "Global table with name: '<TableName>' does not exist."; the emulator gives ResourceNotFoundException with "Requested resource not found: Table: <TableName> not found".
4. Call UpdateTableReplicaAutoScaling(TableName=<the same missing name>, ProvisionedWriteCapacityAutoScalingUpdate={MinimumUnits:1, MaximumUnits:10, ScalingPolicyUpdate:{PolicyName:"test-policy", TargetTrackingScalingPolicyConfiguration:{TargetValue:70.0}}}) and check that it returns the same ResourceNotFoundException code and the same global-table message.
