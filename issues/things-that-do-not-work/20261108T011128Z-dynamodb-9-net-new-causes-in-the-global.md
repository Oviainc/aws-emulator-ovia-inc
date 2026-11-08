Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.create-global-table-already-exists-rejected dynamodb.ddbglobal-legacy-2017-global-table-apis-rejected dynamodb.global-table-already-exists dynamodb.global-table-lifecycle dynamodb.global-table-not-found dynamodb.global-table-settings-index-not-found dynamodb.describe-global-table-not-found-rejected dynamodb.global-table-settings-describe-update dynamodb.global-table-settings-update dynamodb.list-global-tables-reflects-created-tables dynamodb.ddbglobal-replica-updates-local-region-rejected dynamodb.ddbglobal-replica-delete-for-absent-replica-rejected dynamodb.global-table-replica-already-exists-rejected dynamodb.global-table-settings-describe-update-roundtrip dynamodb.deploy-dynamodb-1-cfn-global-table-bidirectional-then-replica-removal-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.create-global-table-already-exists-rejected|run#17|AssertionError|assert message == expected_msg, f"expected message {expected_msg!r}, got {message!r}"
Issue-Key: dynamodb.ddbglobal-legacy-2017-global-table-apis-rejected|run#15|AssertionError|assert message == "One or more parameter values were invalid: DynamoDB global tables version 2017.11.29 is not supported. We recommend using DynamoDB global tab
Issue-Key: dynamodb.global-table-already-exists|run#7|AssertionError|assert first_message == _LEGACY_VERSION_MESSAGE, (
Issue-Key: dynamodb.global-table-lifecycle|run#9|AssertionError|assert create_message == _LEGACY_VERSION_MESSAGE, (
Issue-Key: dynamodb.global-table-not-found|run#8|AssertionError|assert create_message == _LEGACY_VERSION_MESSAGE, (
Issue-Key: dynamodb.global-table-settings-index-not-found|run#10|AssertionError|assert create_message == _LEGACY_VERSION_MESSAGE, (
Issue-Key: dynamodb.describe-global-table-not-found-rejected|run#16|AssertionError|assert real_message == expected_real, f"expected message {expected_real!r}, got {real_message!r}"
Issue-Key: dynamodb.global-table-settings-describe-update|run#18|AssertionError|assert describe_message == expected_message, (
Issue-Key: dynamodb.global-table-settings-update|run#13|AssertionError|assert describe_message == expected_message, (
Issue-Key: dynamodb.list-global-tables-reflects-created-tables|run#16|AssertionError|assert global_name not in after, (
Issue-Key: dynamodb.ddbglobal-replica-updates-local-region-rejected|run#12|AssertionError|assert message == "Cannot add or delete the local region through ReplicaUpdates. Use CreateTable, DeleteTable, or UpdateTable as required.", (
Issue-Key: dynamodb.ddbglobal-replica-delete-for-absent-replica-rejected|run#17|AssertionError|assert absent_message == expected_absent, (
Issue-Key: dynamodb.global-table-replica-already-exists-rejected|run#16|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: dynamodb.global-table-settings-describe-update-roundtrip|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb|PutItem|ResourceNotFoundException
Issue-Commit: f4d58d8ba640bd38bd7af13aafdf9baa78485a99

# dynamodb: 9 net-new causes in the global-table surface — legacy-version, GlobalTableNotFound and ReplicaUpdates refusal messages are emulator-authored, ListGlobalTables leaks 2019.11.21 tables, replica throughput overrides are never reported, and a CFN GlobalTable creates no addressable table

> _Note: this issue was identified against the code base at commit `f4d58d8ba640bd38bd7af13aafdf9baa78485a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGlobalTable

## Description
This issue carries 9 net-new root causes, all in the DynamoDB global-table control plane. Each cause lists the failing check ids it must be closed against.

Cause 1 of 9: the refusal of the legacy 2017.11.29 global-table surface carries emulator-authored wording.
Failing check ids: dynamodb.create-global-table-already-exists-rejected, dynamodb.ddbglobal-legacy-2017-global-table-apis-rejected, dynamodb.global-table-already-exists, dynamodb.global-table-lifecycle, dynamodb.global-table-not-found, dynamodb.global-table-settings-index-not-found.
AWS: Smithy: CreateGlobalTable (GlobalTableName, ReplicationGroup) errors GlobalTableAlreadyExistsException / LimitExceededException / TableNotFoundException / InternalServerError; the version refusal arrives as a ValidationException (the service-common error). Recorded behavior: on accounts/regions that must use version 2019.11.21, the legacy call is refused with ValidationException whose message is fixed text: "One or more parameter values were invalid: DynamoDB global tables version 2017.11.29 is not supported. We recommend using DynamoDB global tables version 2019.11.21, instead of version 2017.11.29 (Legacy)." — no part of it is taken from the request.
Emulator: ValidationException with 'Global tables version 2017.11.29 (Legacy) is no longer supported. Use global tables version 2019.11.21 (Current) instead.'
Why: clients and tooling that match on AWS's "One or more parameter values were invalid: ..." prefix (and on the recommendation wording) do not recognise the refusal, so the migration path from the legacy surface is not surfaced to the caller.
Fix: the CreateGlobalTable handler's validation (and the shared legacy-version refusal it uses) must emit AWS's exact message.

Cause 2 of 9: DescribeGlobalTable's GlobalTableNotFoundException message is truncated.
Failing check ids: dynamodb.describe-global-table-not-found-rejected.
AWS: Smithy: DescribeGlobalTable errors GlobalTableNotFoundException (client, 400, "The specified global table does not exist"). Recorded message, for both a never-created name and a real ACTIVE 2019.11.21 replica-based table the legacy surface cannot see: "Global table not found: Global table with name: '<GlobalTableName>' does not exist." — fixed text around one variable, the GlobalTableName from the request, in single quotes.
Emulator: 'Global table not found: <GlobalTableName>' — the inner "Global table with name: '...' does not exist." clause is missing and the name is unquoted.
Fix: the DescribeGlobalTable handler's not-found error construction.

Cause 3 of 9: DescribeGlobalTableSettings / UpdateGlobalTableSettings use the same truncated not-found message.
Failing check ids: dynamodb.global-table-settings-describe-update, dynamodb.global-table-settings-update.
AWS: Smithy: DescribeGlobalTableSettings and UpdateGlobalTableSettings both model GlobalTableNotFoundException (client, 400). Recorded message: "Global table with name: '<GlobalTableName>' does not exist." — note it has NO "Global table not found: " prefix, unlike DescribeGlobalTable (cause 2); the only variable is the requested name.
Emulator: 'Global table not found: <GlobalTableName>' for both operations.
Fix: the DescribeGlobalTableSettings and UpdateGlobalTableSettings handlers' not-found error construction (these two must not share DescribeGlobalTable's prefixed form).

Cause 4 of 9: ListGlobalTables enumerates 2019.11.21 replica-based tables.
Failing check ids: dynamodb.list-global-tables-reflects-created-tables.
AWS: Smithy: ListGlobalTables returns GlobalTables (GlobalTableList) — it is the 2017.11.29 enumeration. Recorded behavior: a table made global the 2019.11.21-native way (UpdateTable ReplicaUpdates Create, with the replica ACTIVE in the second region) is NOT listed by ListGlobalTables, exactly as DescribeGlobalTable refuses it; the legacy surface has no view of such a table.
Emulator: the table name is present in ListGlobalTables().GlobalTables while its replica is ACTIVE.
Why: callers that enumerate legacy global tables see tables the legacy settings/describe calls then refuse, so inventory and migration tooling built on ListGlobalTables reports state no legacy API can act on.
Fix: the ListGlobalTables handler/state — only tables established through the 2017.11.29 surface belong in that list.

Cause 5 of 9: UpdateTable ReplicaUpdates naming the table's own region is refused with the wrong message.
Failing check ids: dynamodb.ddbglobal-replica-updates-local-region-rejected.
AWS: Smithy: UpdateTable.ReplicaUpdates (ReplicationGroupUpdateList); the refusal is a ValidationException. Recorded message for both the Create and the Delete form naming the table's own region, fixed text with no substituted values: "Cannot add or delete the local region through ReplicaUpdates. Use CreateTable, DeleteTable, or UpdateTable as required."
Emulator: ValidationException 'One or more parameter values were invalid: Cannot create a replica in <region>, which is the Region the table is in'.
Fix: the UpdateTable handler's ReplicaUpdates validation (local-region branch).

Cause 6 of 9: UpdateTable ReplicaUpdates Delete for a region that holds no replica is refused with the wrong message.
Failing check ids: dynamodb.ddbglobal-replica-delete-for-absent-replica-rejected.
AWS: Smithy: UpdateTable.ReplicaUpdates; refusal class ValidationException (ReplicaNotFoundException is modelled only on the settings surface). Recorded message, fixed text with nothing substituted: "Replica specified in the Replica Update or Replica Delete action of the request was not found." It must also differ from the local-region message of cause 5, so the two reasons stay distinguishable.
Emulator: ValidationException "One or more parameter values were invalid: The replica in <region> is not part of this table's replication group".
Fix: the UpdateTable handler's ReplicaUpdates validation (absent-replica branch).

Cause 7 of 9: UpdateTable ReplicaUpdates Create for a region that is already an ACTIVE replica is refused with the wrong message.
Failing check ids: dynamodb.global-table-replica-already-exists-rejected.
AWS: Smithy: UpdateTable.ReplicaUpdates; refusal class ValidationException (ReplicaAlreadyExistsException is reserved for the legacy surface). Recorded message: "Failed to create a the new replica of table with name: \u2018<TableName>\u2019 because one or more replicas already existed as tables." — AWS's own wording including the "a the" and the typographic single quotes; the only variable is the table name from the request.
Emulator: ValidationException "One or more parameter values were invalid: The replica in <region> is already part of this table's replication group".
Fix: the UpdateTable handler's ReplicaUpdates validation (duplicate-replica branch).

Cause 8 of 9: a replica's ProvisionedThroughputOverride is never reported back, so the replica-throughput roundtrip never converges.
Failing check ids: dynamodb.global-table-settings-describe-update-roundtrip.
AWS: Smithy: UpdateTable.ReplicaUpdates carries ProvisionedThroughputOverride, and ReplicaDescription has WarmThroughput (TableWarmThroughputDescription) plus ProvisionedThroughputOverride. Recorded behavior: a read override set at replica create and then changed by a ReplicaUpdates Update is reported by DescribeTable under Table.Replicas[].WarmThroughput.ReadUnitsPerSecond, which settles to the requested value for that replica region.
Emulator: the value never appears — the poll for Replicas[] with ReplicaStatus ACTIVE and WarmThroughput.ReadUnitsPerSecond equal to the requested override never becomes true and the check ends at 'Error: Check timed out after 180.0s'.
Why: a client cannot read back a replica's throughput override at all, so capacity settings applied per replica are unverifiable and any wait-for-settled loop hangs.
Fix: the UpdateTable ReplicaUpdates state and the DescribeTable serializer for ReplicaDescription — store the per-replica override and report it under Replicas[].WarmThroughput.

Cause 9 of 9: a CloudFormation AWS::DynamoDB::GlobalTable that reaches CREATE_COMPLETE leaves no addressable table in the stack's own region.
Failing check ids: dynamodb.deploy-dynamodb-1-cfn-global-table-bidirectional-then-replica-removal-rejected.
AWS: Smithy: PutItem errors ResourceNotFoundException (client, 400) only when the named table does not exist. Recorded behavior: after the stack with one AWS::DynamoDB::GlobalTable (the stack's region plus one replica region) reaches CREATE_COMPLETE, PutItem against that table name in the stack's region succeeds and the item then replicates to the replica region, where DescribeTable reports GlobalTableVersion 2019.11.21 and the peer region ACTIVE.
Emulator: the first PutItem after CREATE_COMPLETE is refused with ResourceNotFoundException 'Requested resource not found' — the declared table does not exist despite the successful stack create.
Why: a GlobalTable declared in IaC reports success while no table exists, so every data-plane call against it fails and the stack's claimed state is false.
Fix: the AWS::DynamoDB::GlobalTable provider and the table state it must create (the table in the stack's region plus its replicas) before the resource is reported CREATE_COMPLETE.

Note: the open issue 20261029T180026Z-dynamodb-global-tables-replica-auto-scaling-indexed-query.md covers 501 NotImplemented answers on other global-table/replica-auto-scaling operations and does not carry any of the replay keys above.
Traces: the failing check ids listed per cause, as examples.

## Evidence
Cause 1 — Expected ValidationException message 'One or more parameter values were invalid: DynamoDB global tables version 2017.11.29 is not supported. We recommend using DynamoDB global tables version 2019.11.21, instead of version 2017.11.29 (Legacy).'; got 'Global tables version 2017.11.29 (Legacy) is no longer supported. Use global tables version 2019.11.21 (Current) instead.'

Cause 2 — Expected GlobalTableNotFoundException message "Global table not found: Global table with name: '<name>' does not exist."; got 'Global table not found: <name>'.

Cause 3 — Expected GlobalTableNotFoundException message "Global table with name: '<name>' does not exist." from DescribeGlobalTableSettings and UpdateGlobalTableSettings; got 'Global table not found: <name>'.

Cause 4 — Expected the 2019.11.21 replica-based table absent from ListGlobalTables even with an ACTIVE replica in the second region; got it present in the returned GlobalTables list.

Cause 5 — Expected ValidationException 'Cannot add or delete the local region through ReplicaUpdates. Use CreateTable, DeleteTable, or UpdateTable as required.'; got "One or more parameter values were invalid: Cannot create a replica in <region>, which is the Region the table is in".

Cause 6 — Expected ValidationException 'Replica specified in the Replica Update or Replica Delete action of the request was not found.'; got "One or more parameter values were invalid: The replica in <region> is not part of this table's replication group".

Cause 7 — Expected ValidationException 'Failed to create a the new replica of table with name: \u2018<table>\u2019 because one or more replicas already existed as tables.'; got "One or more parameter values were invalid: The replica in <region> is already part of this table's replication group".

Cause 8 — Expected DescribeTable Replicas[] for the replica region to report ReplicaStatus ACTIVE and WarmThroughput.ReadUnitsPerSecond equal to the ProvisionedThroughputOverride read capacity (set at create, then updated); got no such value — 'Error: Check timed out after 180.0s'.

Cause 9 — Expected PutItem on the GlobalTable's declared name in the stack's region to succeed after CREATE_COMPLETE; got ClientError ResourceNotFoundException 'Requested resource not found'.

## Steps to Reproduce
1. Cause 1: call CreateGlobalTable (or any 2017.11.29 global-table API) and read the ValidationException message.
2. Cause 2: call DescribeGlobalTable for a never-created name and for a table made global via UpdateTable ReplicaUpdates; read the GlobalTableNotFoundException message.
3. Cause 3: call DescribeGlobalTableSettings and UpdateGlobalTableSettings for such a table; read the message.
4. Cause 4: create a table with a stream, add a replica in a second region via UpdateTable ReplicaUpdates, wait for ReplicaStatus ACTIVE, then call ListGlobalTables.
5. Causes 5-7: call UpdateTable with ReplicaUpdates=[{Create:{RegionName:<own region>}}] and [{Delete:{RegionName:<own region>}}]; with [{Delete:{RegionName:<region with no replica>}}]; and with [{Create:{RegionName:<region already ACTIVE>}}]; compare each code and message.
6. Cause 8: create a PROVISIONED table, register write-capacity auto scaling, add a replica with ProvisionedThroughputOverride ReadCapacityUnits=7, then poll DescribeTable for Replicas[].WarmThroughput.ReadUnitsPerSecond == 7, update the override to 9 and poll again.
7. Cause 9: CreateStack with one AWS::DynamoDB::GlobalTable listing the stack's region plus one replica region, wait for CREATE_COMPLETE, then PutItem into that table name in the stack's region.
8. Or replay the check ids in trace_ids.
