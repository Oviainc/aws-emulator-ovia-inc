Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-instance-core-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-instance-core-lifecycle rds-db-instance-snapshot-and-restore rds-db-cluster-state-roles-and-capacity-operations rds-db-cluster-parameter-group-lifecycle rds-db-proxy-lifecycle rds-db-parameters-pagination
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|ModifyDBInstance|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|RebootDBInstance|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|StopDBInstance|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|StartDBInstance|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateDBSnapshot|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|FailoverDBCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|ResetDBClusterParameterGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|RegisterDBProxyTargets|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|DeregisterDBProxyTargets|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|DescribeDBParameters|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 9919ffd222ff0dfbfa1095fff748926e9311733c

# RDS state-precondition and marker refusals are worded with the emulator's own sentences

> _Note: this issue was identified against the code base at commit `9919ffd222ff0dfbfa1095fff748926e9311733c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBInstance

## Description
Ten refusals for invalid resource state, missing required combinations and a bad pagination marker return AWS's status and error code but a message the emulator writes itself. 10 causes.

Cause 1 of 10: ModifyDBInstance while the instance is not available.
AWS: recorded behavior only — 400 InvalidDBInstanceState, Message 'Database instance is not in available state.' — entirely fixed text, no identifier, no status word.
Emulator: 'DB instance <id> is not available for modification (status <status>)' — interpolates the identifier and the current status AWS omits.
Why: callers matching the documented sentence do not recognise the state refusal.
Fix: the ModifyDBInstance handler's state precondition message.
Traces: rds-db-instance-core-lifecycle#task#3.

Cause 2 of 10: RebootDBInstance while the instance is not rebootable.
AWS: recorded behavior only — 400, Message 'Can only reboot db instances with state in: available, storage-optimization, storage-initialization, incompatible-credentials, incompatible-parameters.  Instance <DBInstanceIdentifier> has state: <status>.' Fixed text: the allowed-state list exactly as worded (including the double space before "Instance"); variable parts are the instance identifier and its current state.
Emulator: 'DB instance <id> is not available for a reboot (status <status>)' — never names the allowed states.
Why: the client loses the allowed-state list AWS supplies.
Fix: the RebootDBInstance handler's state precondition message.
Traces: rds-db-instance-core-lifecycle#task#4.

Cause 3 of 10: StopDBInstance while the instance is not available.
AWS: recorded behavior only — 400, Message 'Instance <DBInstanceIdentifier> is not in available state.' Fixed text: "Instance " + " is not in available state."
Emulator: "DB instance <id> is not available for a stop (status <status>)".
Why: same.
Fix: the StopDBInstance handler's state precondition message.
Traces: rds-db-instance-core-lifecycle#task#5.

Cause 4 of 10: StartDBInstance while the instance is not stopped.
AWS: recorded behavior only — 400, Message "Instance <DBInstanceIdentifier> cannot be started as it is not in one of the following statuses: 'stopped, inaccessible-encryption-credentials-recoverable, incompatible-network (only valid for non-SqlServer instances)'." Fixed text: that whole sentence including the quoted status list; variable part is the identifier.
Emulator: "DB instance <id> is not stopped, so it can't be started (status <status>)" — drops the status list.
Why: same.
Fix: the StartDBInstance handler's state precondition message.
Traces: rds-db-instance-core-lifecycle#task#6.

Cause 5 of 10: CreateDBSnapshot while the instance is not available.
AWS: recorded behavior only — 400, Message 'Cannot create a snapshot because the database instance <DBInstanceIdentifier> is not currently in the available state.'
Emulator: 'DB instance <id> is not available for a snapshot (status <status>)'.
Why: same.
Fix: the CreateDBSnapshot handler's state precondition message.
Traces: rds-db-instance-snapshot-and-restore#task#2.

Cause 6 of 10: FailoverDBCluster on a cluster with fewer than two instances.
AWS: recorded behavior only — 400, Message 'Database cluster:<DBClusterIdentifier> should have at least two database instances for failover.' Fixed text: "Database cluster:" (no space after the colon) + " should have at least two database instances for failover."
Emulator: 'DB cluster <id> has no DB instance to promote to primary' — a different rule and sentence.
Why: the client is told a different precondition than AWS enforces.
Fix: the FailoverDBCluster handler's precondition message.
Traces: rds-db-cluster-state-roles-and-capacity-operations#task#5.

Cause 7 of 10: ResetDBClusterParameterGroup with a pending parameter change.
AWS: recorded behavior only — 400, Message "Cannot reset the parameter group because the parameter '<ParameterName>' has pending changes." Fixed text as quoted; variable part is the parameter name, single-quoted.
Emulator: 'DB cluster parameter group <name> has pending changes to <ParameterName> that require a reboot before it can be reset' — names the group and invents a reboot requirement.
Why: same.
Fix: the ResetDBClusterParameterGroup handler's precondition message.
Traces: rds-db-cluster-parameter-group-lifecycle#task#8.

Cause 8 of 10: RegisterDBProxyTargets with neither DBInstanceIdentifiers nor DBClusterIdentifiers.
AWS: recorded behavior only — 400, Message 'Missing DB instance or DB cluster identifier.' — entirely fixed text.
Emulator: 'name at least one of DBInstanceIdentifiers or DBClusterIdentifiers: a registration with neither registers nothing' — the emulator's own prose, naming modeled member names.
Why: same.
Fix: the RegisterDBProxyTargets handler's input validation message.
Traces: rds-db-proxy-lifecycle#task#13.

Cause 9 of 10: DeregisterDBProxyTargets with neither identifier list.
AWS: recorded behavior only — 400, Message 'Missing DB instance or DB cluster identifier.'
Emulator: 'name at least one of DBInstanceIdentifiers or DBClusterIdentifiers: a deregistration with neither deregisters nothing'.
Why: same.
Fix: the DeregisterDBProxyTargets handler's input validation message.
Traces: rds-db-proxy-lifecycle#task#14.

Cause 10 of 10: DescribeDBParameters with an unparseable Marker.
AWS: Smithy DescribeDBParameters is paginated on Marker/Marker. Recorded: 400, Message 'Invalid marker: <Marker>. Please use the marker returned from the prior request or omit the marker parameter.' Fixed text: "Invalid marker: " + ". Please use the marker returned from the prior request or omit the marker parameter."; the variable part is the offending Marker echoed from the request.
Emulator: 'Invalid marker. Please use the marker returned from a prior request or omit the marker parameter.' — never echoes the marker and says "a prior request" instead of "the prior request".
Why: the client cannot see which marker was rejected.
Fix: the DescribeDBParameters handler's marker validation message.
Traces: rds-db-parameters-pagination#task#7.

## Evidence
Replay cluster details, expected vs actual $.ErrorResponse.Error.Message (every status matched; only Message diverged):
1. rds-db-instance-core-lifecycle#task#3 ModifyDBInstance 400: expected 'Database instance is not in available state.', got 'DB instance sim2real-probe-rds-inst-61aaf05a is not available for modification (status creating)'.
2. #task#4 RebootDBInstance 400: expected 'Can only reboot db instances with state in: available, storage-optimization, storage-initialization, incompatible-credentials, incompatible-parameters.  Instance <id> has state: creating.', got 'DB instance <id> is not available for a reboot (status creating)'.
3. #task#5 StopDBInstance 400: expected 'Instance <id> is not in available state.', got 'DB instance <id> is not available for a stop (status creating)'.
4. #task#6 StartDBInstance 400: expected "Instance <id> cannot be started as it is not in one of the following statuses: 'stopped, inaccessible-encryption-credentials-recoverable, incompatible-network (only valid for non-SqlServer instances)'.", got "DB instance <id> is not stopped, so it can't be started (status creating)".
5. rds-db-instance-snapshot-and-restore#task#2 CreateDBSnapshot 400: expected 'Cannot create a snapshot because the database instance <id> is not currently in the available state.', got 'DB instance <id> is not available for a snapshot (status creating)'.
6. rds-db-cluster-state-roles-and-capacity-operations#task#5 FailoverDBCluster 400: expected 'Database cluster:<id> should have at least two database instances for failover.', got 'DB cluster <id> has no DB instance to promote to primary'.
7. rds-db-cluster-parameter-group-lifecycle#task#8 ResetDBClusterParameterGroup 400: expected "Cannot reset the parameter group because the parameter 'max_connections' has pending changes.", got 'DB cluster parameter group <name> has pending changes to max_connections that require a reboot before it can be reset'.
8. rds-db-proxy-lifecycle#task#13 RegisterDBProxyTargets 400: expected 'Missing DB instance or DB cluster identifier.', got 'name at least one of DBInstanceIdentifiers or DBClusterIdentifiers: a registration with neither registers nothing'.
9. rds-db-proxy-lifecycle#task#14 DeregisterDBProxyTargets 400: expected 'Missing DB instance or DB cluster identifier.', got the deregistration variant of the same emulator prose.
10. rds-db-parameters-pagination#task#7 DescribeDBParameters 400: expected 'Invalid marker: !!!!. Please use the marker returned from the prior request or omit the marker parameter.', got 'Invalid marker. Please use the marker returned from a prior request or omit the marker parameter.'

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-run rds-db-instance-core-lifecycle#task#3 and every step in trace_ids; each drives the resource into the offending state (or sends the offending input) and compares $.ErrorResponse.Error.Message.
