Issue-Source: trace-sweep-divergence
Issue-Trace: rds-global-cluster-delete-with-member
Issue-Step: task#7
Issue-Severity: CRITICAL
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-global-cluster-delete-with-member rds-create-db-instance-catalog-triple rds-identifier-validation-and-lowercasing
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateDBCluster|2xx->4xx|$.CreateDBClusterResponse
Issue-Key: rds|CreateDBCluster|2xx->4xx|status
Issue-Key: rds|CreateDBInstance|2xx->4xx|$.CreateDBInstanceResponse
Issue-Key: rds|CreateDBInstance|2xx->4xx|status
Issue-Key: rds|CreateDBSecurityGroup|2xx->4xx|$.CreateDBSecurityGroupResponse
Issue-Key: rds|CreateDBSecurityGroup|2xx->4xx|status
Issue-Key: rds|DeleteDBSecurityGroup|2xx->4xx|status
Issue-Commit: 9919ffd222ff0dfbfa1095fff748926e9311733c

# RDS refuses creates AWS accepts: global-cluster member, instance class, security group name

> _Note: this issue was identified against the code base at commit `9919ffd222ff0dfbfa1095fff748926e9311733c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
Four steps that AWS answers 200 are refused by the emulator, so the fixtures cannot even set up the resources their later steps exercise. Each is an over-strict guard, the mirror image of the missing-validation bundle. 4 causes.

Cause 1 of 4: CreateDBCluster joining an existing global cluster (GlobalClusterIdentifier supplied, engine aurora-mysql, explicit DBSubnetGroup).
AWS: 200 with CreateDBClusterResult.DBCluster — Status 'creating', the supplied GlobalClusterIdentifier echoed, DBSubnetGroup set, EngineVersion 8.0.mysql_aurora.3.10.3, DBClusterParameterGroup default.aurora-mysql8.0, Port 3306, the cluster's TagList and HostedZoneId.
Emulator: 400 with no CreateDBClusterResponse at all.
Why: a member cluster cannot be added to a global cluster, so global-database topologies are unreachable and the fixture's later delete-with-member assertions never run.
Fix: the CreateDBCluster handler's GlobalClusterIdentifier path.
Traces: rds-global-cluster-delete-with-member#task#7.

Cause 2 of 4: CreateDBInstance with DBInstanceClass db.t4g.micro, Engine mysql 8.0.40 in an explicit subnet group.
AWS: 200 with CreateDBInstanceResult.DBInstance — DBInstanceStatus 'creating', DBInstanceClass db.t4g.micro, the resolved DBSubnetGroup with both subnets and their AZs, VpcSecurityGroups, DBParameterGroups default.mysql8.0, PendingModifiedValues.MasterUserPassword.
Emulator: 400 with no CreateDBInstanceResponse.
Why: a supported instance class is rejected, so instance-creation flows fail; note the emulator simultaneously ACCEPTS the db.t2.micro combination AWS refuses (see the missing-validation bundle), so its compatibility table is inverted for this engine.
Fix: the CreateDBInstance handler's class/engine compatibility table.
Traces: rds-create-db-instance-catalog-triple#task#12.

Cause 3 of 4: CreateDBSecurityGroup with a DBSecurityGroupName containing a leading double hyphen and a trailing hyphen.
AWS: Smithy CreateDBSecurityGroup input DBSecurityGroupName is a plain String with no @pattern — the identifier rule RDS applies to parameter-group names is NOT applied here. Recorded: 200 with CreateDBSecurityGroupResult.DBSecurityGroup echoing the name, its DBSecurityGroupArn, VpcId, empty IPRanges/EC2SecurityGroups, the description and OwnerId.
Emulator: 400 — it applies the parameter-group identifier rule to a member the model does not constrain.
Why: legitimate security-group names are rejected.
Fix: the CreateDBSecurityGroup handler's name validation (drop the borrowed identifier pattern).
Traces: rds-identifier-validation-and-lowercasing#task#14.

Cause 4 of 4: DeleteDBSecurityGroup at teardown for that group.
AWS: 200.
Emulator: 404 — the group was never created (a consequence of cause 3, but a separate step and response).
Why: teardown fails and the fixture leaks.
Fix: resolved by cause 3; re-verify the DeleteDBSecurityGroup path.
Traces: rds-identifier-validation-and-lowercasing#teardown#15.

## Evidence
Replay cluster details:
1. rds-global-cluster-delete-with-member#task#7 CreateDBCluster: status expected 200 got 400; $.CreateDBClusterResponse missing (recording carried DBCluster with Status 'creating', GlobalClusterIdentifier rdserr-gm-gc-a599b675, DBSubnetGroup rdserr-gm-sn-d8f8a272, DBClusterParameterGroup 'default.aurora-mysql8.0', EngineVersion '8.0.mysql_aurora.3.10.3', Port '3306').
2. rds-create-db-instance-catalog-triple#task#12 CreateDBInstance: status expected 200 got 400; $.CreateDBInstanceResponse missing (recording carried DBInstance with DBInstanceClass 'db.t4g.micro', EngineVersion '8.0.40', DBSubnetGroup vpc-00000000000000022 with subnets in us-west-2a/us-west-2b, DBParameterGroups default.mysql8.0).
3. rds-identifier-validation-and-lowercasing#task#14 CreateDBSecurityGroup: status expected 200 got 400; $.CreateDBSecurityGroupResponse missing (recording carried DBSecurityGroup DBSecurityGroupName 'rdserr--sg-59a9a1f4-', DBSecurityGroupArn arn:aws:rds:us-west-2:000000000000:secgrp:rdserr--sg-59a9a1f4-, OwnerId '000000000000').
4. rds-identifier-validation-and-lowercasing#teardown#15 DeleteDBSecurityGroup: status expected 200 got 404.
(Generated names, vpc/subnet ids and the account id above are this run's values.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-run rds-global-cluster-delete-with-member#task#7 and every step in trace_ids; each sends a create (or the matching teardown delete) that the recording answers 200 and the replay compares status and the success envelope.
