Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-instance-core-lifecycle-and-features
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-instance-core-lifecycle-and-features rds-db-instance-target-error-paths rds-db-tenant-database-and-restore-error-paths rds-tenant-databases rds-operations-on-missing-cluster rds-sg-ingress-lifecycle-and-errors rds-db-cluster-state-roles-and-capacity-operations
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeValidDBInstanceModifications|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateTenantDatabase|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|ModifyDBCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|RevokeDBSecurityGroupIngress|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|DescribeDBClusterAutomatedBackups|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Related: 20261029T214632Z-rds-already-exists-conflict-refusals-carry-the-emulator.md
Issue-Commit: 9919ffd222ff0dfbfa1095fff748926e9311733c

# RDS not-found refusals for instances, clusters, subnet and security groups use emulator wording

> _Note: this issue was identified against the code base at commit `9919ffd222ff0dfbfa1095fff748926e9311733c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
Six more 404 refusals use the emulator's central '<Type> <id> not found.' template instead of AWS's per-resource sentence. Status and error code match; only the Message diverges. 6 causes.

Cause 1 of 6: CreateDBInstance naming a DB subnet group that does not exist.
AWS: recorded behavior only — 404, Message "DBSubnetGroup '<DBSubnetGroupName>' not found." Fixed text: "DBSubnetGroup '" + "' not found."; variable part is the request's DBSubnetGroupName, single-quoted, and the type name is the modeled shape name with no internal space.
Emulator: 404 with "DB Subnet group '<name>' not found." — a space-separated, differently-cased type name.
Why: callers matching AWS's sentence miss the missing-subnet-group case.
Fix: the CreateDBInstance handler's subnet-group existence check message.
Traces: rds-db-instance-core-lifecycle-and-features#task#1.

Cause 2 of 6: DescribeValidDBInstanceModifications on a missing instance.
AWS: recorded behavior only — 404, Message 'DBInstance not found: <DBInstanceIdentifier>' — fixed prefix "DBInstance not found: " then the request identifier.
Emulator: 'DBInstance <id> not found.' — identifier and phrase order inverted, terminal period added.
Why: same.
Fix: the DescribeValidDBInstanceModifications handler's existence check.
Traces: rds-db-instance-core-lifecycle-and-features#task#10; also rds-db-instance-target-error-paths#task#12.

Cause 3 of 6: CreateTenantDatabase on a missing instance.
AWS: recorded behavior only — 404, Message 'DBInstance not found: <DBInstanceIdentifier>'.
Emulator: 'DBInstance <id> not found.'
Why: same.
Fix: the CreateTenantDatabase handler's existence check.
Traces: rds-db-tenant-database-and-restore-error-paths#task#1; also rds-tenant-databases#task#2.

Cause 4 of 6: ModifyDBCluster on a missing cluster.
AWS: recorded behavior only — 404, Message 'DBCluster not found' — entirely fixed, no identifier and no terminal period.
Emulator: 'DBCluster <DBClusterIdentifier> not found.' — interpolates the identifier AWS omits.
Why: the emulator leaks a request value into a constant message.
Fix: the ModifyDBCluster handler's existence check.
Traces: rds-operations-on-missing-cluster#task#2.

Cause 5 of 6: RevokeDBSecurityGroupIngress on a missing security group.
AWS: recorded behavior only — 404, Message 'DBSecurityGroup not found: <DBSecurityGroupName>'.
Emulator: 'DBSecurityGroup <name> not found.'
Why: same inverted template.
Fix: the RevokeDBSecurityGroupIngress handler's existence check.
Traces: rds-sg-ingress-lifecycle-and-errors#task#5.

Cause 6 of 6: DescribeDBClusterAutomatedBackups for a cluster resource id with no backup.
AWS: recorded behavior only — 404, Message 'DBClusterAutomatedBackup for DBCluster <DbClusterResourceId> not found.' Fixed text: "DBClusterAutomatedBackup for DBCluster " + " not found."; variable part is the requested cluster resource id.
Emulator: 'No automated backup for DB cluster <id> was found.' — an entirely different sentence.
Why: same message-matching break.
Fix: the DescribeDBClusterAutomatedBackups handler's existence check.
Traces: rds-db-cluster-state-roles-and-capacity-operations#task#11.

## Evidence
Replay cluster details, expected vs actual $.ErrorResponse.Error.Message (status 404 matched in all six):
1. rds-db-instance-core-lifecycle-and-features#task#1 CreateDBInstance: expected "DBSubnetGroup 'sim2real-probe-rds-subnetgrp-missing-c6a7d4cb' not found.", got "DB Subnet group 'sim2real-probe-rds-subnetgrp-missing-c6a7d4cb' not found."
2. rds-db-instance-core-lifecycle-and-features#task#10 DescribeValidDBInstanceModifications: expected 'DBInstance not found: sim2real-probe-rds-dbinst-missing-c6a7d4cb', got 'DBInstance sim2real-probe-rds-dbinst-missing-c6a7d4cb not found.'
3. rds-db-tenant-database-and-restore-error-paths#task#1 CreateTenantDatabase: expected 'DBInstance not found: sim2real-probe-rds-dbinst-missing-1cd648d7', got 'DBInstance ... not found.'
4. rds-operations-on-missing-cluster#task#2 ModifyDBCluster: expected 'DBCluster not found', got 'DBCluster sim2real-probe-rds-dbcluster-missing-d0040235 not found.'
5. rds-sg-ingress-lifecycle-and-errors#task#5 RevokeDBSecurityGroupIngress: expected 'DBSecurityGroup not found: sim2real-probe-rds-missing-sg', got 'DBSecurityGroup sim2real-probe-rds-missing-sg not found.'
6. rds-db-cluster-state-roles-and-capacity-operations#task#11 DescribeDBClusterAutomatedBackups: expected 'DBClusterAutomatedBackup for DBCluster cluster-ZZZZZZZZZZZZZZZZZZZZZZZZZZ not found.', got 'No automated backup for DB cluster cluster-ZZZZZZZZZZZZZZZZZZZZZZZZZZ was found.'

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-run rds-db-instance-core-lifecycle-and-features#task#1 and the steps in trace_ids; each targets an identifier that was never created and compares $.ErrorResponse.Error.Message.
