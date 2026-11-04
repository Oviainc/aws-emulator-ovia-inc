Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.create-db-subnet-group-single-az-rejected rds.subnet-group-insufficient-azs rds.deploy-db-subnet-group-cfn-instance-placement rds.deploy-rds-subnet-group-cfn-az-coverage rds.deploy-cfn-subnet-group-drives-instance-placement rds.default-max-connections-follows-instance-class-memory-formula rds.describe-option-groups-list-all rds.deep-rds-sql-truth-force-ssl-plaintext-login-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.create-db-subnet-group-single-az-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: rds.subnet-group-insufficient-azs|run#13|AssertionError|assert message == (
Issue-Key: rds.deploy-db-subnet-group-cfn-instance-placement|run#16|AssertionError|assert group_azs == chosen_azs, (
Issue-Key: rds.deploy-rds-subnet-group-cfn-az-coverage|run#12|AssertionError|assert actual_azs == set(vpc["azs"]), f"subnet group AZs {actual_azs} != expected {vpc['azs']}"
Issue-Key: rds.deploy-cfn-subnet-group-drives-instance-placement|run#13|KeyError|inst_subnets = {s["SubnetIdentifier"] for s in instance["DBSubnetGroup"]["Subnets"]}
Issue-Key: rds|DescribeDBParameters|DBParameterGroupNotFound
Issue-Key: rds.deep-rds-sql-truth-force-ssl-plaintext-login-rejected|run#14|AssertionError|assert stored.get("ApplyType") == "dynamic", (
Issue-Related: 20261029T195009Z-rds-6-net-new-root-causes-copydbparametergroup-copydbsnapshot.md
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# rds: 5 net-new causes — DB subnet groups report the wrong availability zones and omit Subnets, and the engine default parameter/option-group catalogs are empty

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBSubnetGroup

## Description
Five distinct causes around subnet/AZ resolution and the engine default catalogs.

Cause 1 of 5: a subnet's availability zone is not resolved from EC2 — the emulator assigns its own. AWS: CreateDBSubnetGroup resolves each SubnetIds entry against EC2 and reports DBSubnetGroup$Subnets[].SubnetAvailabilityZone$Name as that subnet's actual AZ; the AZ-coverage refusal names the AZs actually covered: "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: <comma-separated AZs of the supplied subnets>. Add subnets to cover at least 2 AZs." (Smithy: DBSubnetGroupDoesNotCoverEnoughAZs, httpError 400). Emulator: the AZ it names is not the AZ of the supplied subnets (a group built from subnets in us-west-2a is refused with 'Current AZ coverage: us-west-2b'), and a group built from two subnets in two different AZs is reported as covering a single AZ. Checks: rds.create-db-subnet-group-single-az-rejected, rds.db-subnet-group-insufficient-azs, rds.subnet-group-insufficient-azs, rds.deploy-db-subnet-group-cfn-instance-placement, rds.deploy-rds-subnet-group-cfn-az-coverage.

Cause 2 of 5: DescribeDBSubnetGroups omits the modeled Subnets member. AWS: DBSubnetGroup$Subnets is a list of Subnet (SubnetIdentifier, SubnetAvailabilityZone, SubnetOutpost, SubnetStatus). Emulator: the member is absent, so a client indexing response['DBSubnetGroups'][0]['Subnets'] raises KeyError. Check: rds.deploy-cfn-subnet-group-drives-instance-placement.

Cause 3 of 5: the account has no engine default DB parameter groups. AWS pre-creates 'default.<engine><major>' (e.g. default.postgres17) in every account, and DescribeDBParameters on it returns the engine's parameters. Emulator: DescribeDBParameters(DBParameterGroupName='default.postgres17') answers DBParameterGroupNotFound (404) 'DBParameterGroup not found: default.postgres17' even though CreateDBInstance reports that very name in DBInstance$DBParameterGroups. Check: rds.default-max-connections-follows-instance-class-memory-formula.

Cause 4 of 5: the account has no option-group catalog. AWS: DescribeOptionGroups with no filter returns the service-provided default option groups ('default:<engine>-<major>'), the same names CreateDBInstance reports in OptionGroupMemberships. Emulator: the list comes back empty. Check: rds.describe-option-groups-list-all.

Cause 5 of 5: engine parameter metadata is missing, so a modifiable dynamic parameter reads back with no ApplyType. AWS: DescribeDBParameters/DescribeEngineDefaultParameters report rds.force_ssl for postgres17 with ApplyType 'dynamic', Source 'engine-default', DataType 'boolean' and AllowedValues '0,1'. Emulator: the parameter's ApplyType is None. Check: rds.deep-rds-sql-truth-force-ssl-plaintext-login-rejected.

Why: AZ coverage is the rule that decides whether an instance can be created at all and where it is placed, so a wrong AZ makes a valid two-AZ group refused and an invalid one-AZ group accepted; a missing Subnets member breaks every client that inspects placement; and with no default parameter/option groups a client cannot read or reason about the settings the instance it just created claims to use. Fix: subnet→AZ resolution in create_db_subnet_group.rs/modify_db_subnet_group.rs and the DBSubnetGroup serializer in the rds crate, plus the account-seeding of default DB parameter groups and option groups and the engine parameter catalog (ApplyType/Source/AllowedValues) behind DescribeDBParameters/DescribeOptionGroups. Related open issues 20261029T214632Z-rds-db-subnet-group-az-coverage-rule-is.md, 20261030T001542Z-rds-db-subnet-group-and-db-instance-projections.md and 20261030T001542Z-rds-engine-parameter-and-event-category-catalogs-are.md describe neighbouring defects and do not carry these replay keys.

## Evidence
Verbatim replay output:
- 'expected message "... Current AZ coverage: us-west-2a. Add subnets to cover at least 2 AZs.", got "... Current AZ coverage: us-west-2b. Add subnets to cover at least 2 AZs."' (create-db-subnet-group-single-az-rejected); the same us-west-2b sentence for db-subnet-group-insufficient-azs and subnet-group-insufficient-azs.
- 'DBSubnetGroup <name> covers {us-west-2b}, expected exactly {us-west-2a, us-west-2b}' and "subnet group AZs {'us-west-2a','us-west-2b'} != expected ['us-west-2a','us-west-2b','us-west-2c']".
- "KeyError: 'Subnets'" (deploy-cfn-subnet-group-drives-instance-placement).
- 'botocore.errorfactory.DBParameterGroupNotFoundFault: An error occurred (DBParameterGroupNotFound) when calling the DescribeDBParameters operation: DBParameterGroup not found: default.postgres17'.
- 'expected multiple catalog option groups, got []'.
- 'rds.force_ssl must be a dynamic parameter in postgres17; got None'.

## Steps to Reproduce
1. ec2.create_subnet in two distinct AZs, then rds.create_db_subnet_group(SubnetIds=[both]) and describe_db_subnet_groups — the reported SubnetAvailabilityZone/AZ set does not match the subnets' AZs; repeat with both subnets in one AZ and read the refusal's 'Current AZ coverage' list.
2. describe_db_subnet_groups(DBSubnetGroupName=<group>) and read ['Subnets'] — KeyError.
3. create_db_instance(Engine='postgres') then describe_db_parameters(DBParameterGroupName='default.postgres17') — 404 DBParameterGroupNotFound.
4. describe_option_groups() with no filter — empty list.
5. describe_db_parameters on the postgres17 default group and read rds.force_ssl's ApplyType — absent.
