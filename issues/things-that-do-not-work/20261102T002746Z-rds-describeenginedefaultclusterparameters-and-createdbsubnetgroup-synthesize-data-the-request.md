Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-parameter-group-lifecycle-and-engine-defaults
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-parameter-group-lifecycle-and-engine-defaults rds-subnet-group-az-coverage-and-naming
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeEngineDefaultClusterParameters|2xx->2xx|$.DescribeEngineDefaultClusterParametersResponse.DescribeEngineDefaultClusterParametersResult.EngineDefaults.Parameters.Parameter[*].SupportedEngineModes.member
Issue-Key: rds|CreateDBSubnetGroup|2xx->2xx|$.CreateDBSubnetGroupResponse.CreateDBSubnetGroupResult.DBSubnetGroup.Subnets.Subnet[*].SubnetAvailabilityZone.Name
Issue-Related: 20261029T214632Z-rds-omits-vpc-subnet-and-availability-zone-attributes.md
Issue-Related: 20261030T001542Z-rds-engine-parameter-and-event-category-catalogs-are.md
Issue-Commit: 67befbd38f0c0c0cf22099d8dd1549ab186d2816

# rds DescribeEngineDefaultClusterParameters and CreateDBSubnetGroup synthesize data the request and state already determine

> _Note: this issue was identified against the code base at commit `67befbd38f0c0c0cf22099d8dd1549ab186d2816`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEngineDefaultClusterParameters

## Description
Two read paths answer 200 with values the emulator invents instead of the ones the request and the stored state determine: the Aurora cluster parameter-family default catalog, and the VPC/Availability-Zone detail of the subnets named in a CreateDBSubnetGroup request. 2 causes; each must be closed and re-replayed separately.

Cause 1 of 2 — DescribeEngineDefaultClusterParameters (fixture rds-db-parameter-group-lifecycle-and-engine-defaults, step task#11).
AWS: Smithy (com.amazonaws.rds#EngineDefaults) models DBParameterGroupFamily, Parameters (ParametersList) and Marker, where Marker is "An optional pagination token provided by a previous EngineDefaults request. If this parameter is specified, the response includes only records beyond the marker, up to the value specified by MaxRecords". For the Aurora MySQL cluster parameter family AWS returns that family's real cluster-level parameter catalog, ordered by parameter name, one page of MaxRecords entries plus an opaque Marker when entries remain. The recorded page holds 20 Parameter entries beginning with aurora_binlog_read_buffer_size: DataType integer, ApplyType dynamic, AllowedValues '8192-536870912', ParameterValue '5242880', MinimumEngineVersion '5.7.mysql_aurora.2.04.5', Description 'Read buffer size used by master dump thread when the switch aurora_binlog_use_large_read_buffer is ON.', and SupportedEngineModes holding the single value 'provisioned'. Those are per-family catalog facts, not per-run values; the Marker's literal bytes are an opaque token, so the requirement is that a Marker is present when the page is truncated, not that exact string.
Emulator: answers 200 with a short generic MySQL server-parameter list instead of the Aurora cluster parameter family - the first entry is binlog_format (ApplyType static, DataType string, AllowedValues 'ROW,STATEMENT,MIXED,OFF', ParameterValue 'OFF'), no entry carries MinimumEngineVersion, entries 4 through 19 of the recorded page are absent altogether, SupportedEngineModes carries several modes where AWS carries only 'provisioned', and EngineDefaults.Marker is omitted so the listing claims to be complete.
Why: this call is how a client discovers what a cluster parameter may be set to - its data type, its allowed range, whether it applies dynamically or needs a reboot, and the minimum engine version that supports it. Against the emulator a client validating a ModifyDBClusterParameterGroup value against the defaults sees the wrong parameter names, the wrong data types and ranges, and no continuation token, so it both validates against the wrong rules and silently stops after one short page.
Fix: the DescribeEngineDefaultClusterParameters handler (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_engine_default_cluster_parameters.rs) and the engine-default parameter catalog it reads in the crate's state: the cluster-parameter family must have its own catalog with ApplyType/DataType/AllowedValues/MinimumEngineVersion/SupportedEngineModes per parameter, and the handler must page it and emit Marker. Related open issues that do NOT carry this step's replay key: 20261030T001542Z-rds-engine-parameter-and-event-category-catalogs-are.md (owns the DescribeEngineDefaultParameters sibling at task#10) and 20261030T001542Z-rds-parameter-pagination-markers-are-raw-parameter-names.md (marker encoding).
Traces: rds-db-parameter-group-lifecycle-and-engine-defaults#task#11.

Cause 2 of 2 — CreateDBSubnetGroup (fixture rds-subnet-group-az-coverage-and-naming, step task#9; same cluster also at task#16 and task#17).
AWS: Smithy (com.amazonaws.rds#DBSubnetGroup) models VpcId as "Provides the VpcId of the DB subnet group" and Subnets as a list of com.amazonaws.rds#Subnet{SubnetIdentifier, SubnetAvailabilityZone, SubnetStatus}. AWS resolves both from the SubnetIds supplied in the request: VpcId is the VPC that owns those subnets, and each Subnet's SubnetAvailabilityZone.Name is that individual subnet's own Availability Zone. The recorded request supplies subnets in two zones and gets back Subnet[0] in us-west-2a and Subnet[1] in us-west-2b, with VpcId equal to the VPC those subnets belong to.
Emulator: answers 200 but reports the SAME Availability Zone for every member - Subnets.Subnet[1].SubnetAvailabilityZone.Name comes back us-west-2a where the subnet is in us-west-2b - and reports a VpcId that is not the VPC of the supplied subnets but an id minted by the handler. (The literal VPC id differs per environment and is not itself the requirement; that the id is the one owning the named subnets is.)
Why: the Availability Zones covered by a DB subnet group are what RDS uses to place a Multi-AZ cluster or instance. A client that creates a two-AZ group and reads the response back sees single-AZ coverage and a VPC it did not use, so it cannot verify its own placement precondition, and the service's own AZ-coverage rule cannot be computed correctly from the stored group (see related open issues 20261029T214632Z-rds-db-subnet-group-az-coverage-rule-is.md and 20261030T001542Z-rds-db-subnet-group-and-db-instance-projections.md, neither of which carries this step's replay key).
Fix: the CreateDBSubnetGroup handler (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_subnet_group.rs) and the subnet-group record it writes to state: resolve each supplied subnet id to its own Availability Zone and take the group's VpcId from the VPC that owns them, instead of stamping one zone onto every member and generating a fresh VpcId.
Traces: rds-subnet-group-az-coverage-and-naming#task#9 (cluster members task#16, task#17).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true).

Cause 1 — rds-db-parameter-group-lifecycle-and-engine-defaults#task#11, DescribeEngineDefaultClusterParameters, expected 200 / actual 200:
  $...EngineDefaults.Marker: expected a continuation token ('YXdzX2RlZmF1bHRfc2FnZW1ha2VyX3JvbGU=' in this recording), got None
  $...EngineDefaults.Parameters.Parameter[0].ParameterName: expected 'aurora_binlog_read_buffer_size', got 'binlog_format'
  $...Parameter[0].DataType: expected 'integer', got 'string'
  $...Parameter[0].ApplyType: expected 'dynamic', got 'static'
  $...Parameter[0].AllowedValues: expected '8192-536870912', got 'ROW,STATEMENT,MIXED,OFF'
  $...Parameter[0].ParameterValue: expected '5242880', got 'OFF'
  $...Parameter[0].MinimumEngineVersion: expected '5.7.mysql_aurora.2.04.5', got None
  $...Parameter[0].Description: expected 'Read buffer size used by master dump thread when the switch aurora_binlog_use_large_read_buffer is ON.', got 'The format binary log events are written in; OFF disables binary logging.'
  $...Parameter[0].SupportedEngineModes.member: type_mismatch — expected the single value 'provisioned', got a multi-value list
  $...Parameter[4] .. Parameter[19]: missing entirely (the emulator's page is shorter than the recorded one)
  (Parameter[1], [2], [3] diverge on the same member set.)
  Model: com.amazonaws.rds#EngineDefaults{DBParameterGroupFamily, Marker, Parameters}; Marker documented as the EngineDefaults pagination token.

Cause 2 — rds-subnet-group-az-coverage-and-naming#task#9, CreateDBSubnetGroup, expected 200 / actual 200 (cluster also covers task#16, task#17):
  $...DBSubnetGroup.Subnets.Subnet[1].SubnetAvailabilityZone.Name: expected 'us-west-2b', got 'us-west-2a'
  $...DBSubnetGroup.VpcId: expected the VPC owning the supplied subnets ('vpc-00000000000000024' in this recording), got an unrelated generated id ('vpc-cd3f34b29191d275e')
  Model: com.amazonaws.rds#DBSubnetGroup{VpcId, Subnets: SubnetList}, com.amazonaws.rds#Subnet{SubnetIdentifier, SubnetAvailabilityZone, SubnetStatus}.

The recorded parameter values, marker bytes and VPC id above are this recording's instance of each rule; the requirements are the catalog/metadata of the requested parameter family, the presence of a Marker on a truncated page, and VpcId/SubnetAvailabilityZone derived from the subnets named in the request.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Cause 1: replay fixture rds-db-parameter-group-lifecycle-and-engine-defaults and stop at step task#11 — DescribeEngineDefaultClusterParameters for the Aurora MySQL cluster parameter family returns a generic MySQL parameter list starting at binlog_format, with no MinimumEngineVersion, fewer entries than the recorded page, and no EngineDefaults.Marker. Equivalent by hand: aws rds describe-engine-default-cluster-parameters --db-parameter-group-family aurora-mysql8.0 and compare the first 20 parameters and the Marker against AWS.
3. Cause 2: replay fixture rds-subnet-group-az-coverage-and-naming and stop at step task#9 — CreateDBSubnetGroup with subnets in two Availability Zones returns both members in us-west-2a and a VpcId unrelated to those subnets. Equivalent by hand: aws rds create-db-subnet-group --db-subnet-group-name <name> --db-subnet-group-description d --subnet-ids <subnet-in-az-a> <subnet-in-az-b> and compare each Subnets[].SubnetAvailabilityZone.Name and VpcId with the subnets supplied.
