Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: docdb.subnet-group-rejects-insufficient-az-coverage
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb.subnet-group-rejects-insufficient-az-coverage|_error#14|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: 9ba218dfac78ec8aaaf32d0b8457457c6bd04b91

# DocDB CreateDBSubnetGroup never resolves subnets against EC2 state, so a single-AZ subnet group is accepted instead of raising DBSubnetGroupDoesNotCoverEnoughAZs (and the stored group carries no VpcId/SubnetAvailabilityZone)

> _Note: this issue was identified against the code base at commit `9ba218dfac78ec8aaaf32d0b8457457c6bd04b91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBSubnetGroup

## Description
One cause (Cause 1 of 1): DocDB's CreateDBSubnetGroup performs no Availability-Zone coverage validation, because it never resolves the supplied SubnetIds against the account's EC2 subnet state at all.

AWS: Smithy (com.amazonaws.docdb#AmazonRDSv19) binds CreateDBSubnetGroup with input {DBSubnetGroupName, DBSubnetGroupDescription, SubnetIds: SubnetIdentifierList, Tags} and models the error DBSubnetGroupDoesNotCoverEnoughAZs (errorKind client, httpError 400, awsQueryError code 'DBSubnetGroupDoesNotCoverEnoughAZs'), documented as "Subnets in the subnet group should cover at least two Availability Zones unless there is only one Availability Zone." AWS therefore rejects a CreateDBSubnetGroup whose subnets all live in one AZ (in a Region offering two or more AZs) with HTTP 400, awsQuery Error.Code 'DBSubnetGroupDoesNotCoverEnoughAZs', and Error.Message of the form: fixed text "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: " + the variable part (the comma-separated list of AZ names the requested subnets actually cover, taken from EC2 state for the requested SubnetIds) + fixed text ". Add subnets to cover at least 2 AZs." In the replayed check the variable part was the single zone the two requested subnets shared; that zone name is a value from the request/state, not part of the template. A rejected create must also leave no group behind (DescribeDBSubnetGroups must not list the name). The same model shows DBSubnetGroup members VpcId ("Provides the virtual private cloud (VPC) ID of the subnet group") and Subnet.SubnetAvailabilityZone ("Specifies the Availability Zone for the subnet"), both of which AWS emits because the service resolves each subnet to its VPC and AZ.

Emulator: the single-AZ create SUCCEEDS with HTTP 200 and returns a DBSubnetGroup with SubnetGroupStatus 'Complete' and the two same-AZ subnets as Active. No error is raised. The returned envelope also carries no 'VpcId' member and each Subnets entry carries only {SubnetIdentifier, SubnetStatus} with no 'SubnetAvailabilityZone' — i.e. the handler stores the caller's subnet id strings verbatim and never looks them up in EC2, which is precisely why the AZ-coverage precondition cannot be evaluated. (Related but distinct: the open issue '20261024T030130Z-rds-state-and-validation-fidelity-ca-certificate-modification' Cause 4 reports the missing DBSubnetGroup.VpcId on the rds service; this finding is the docdb service's CreateDBSubnetGroup AZ-coverage precondition and is not covered by it.)

Why: a client that builds a DocDB cluster's networking relies on this refusal to catch a mis-specified subnet list before provisioning. Against the emulator the bad group is accepted and looks 'Complete', so the fault surfaces later (or never) and any client branching on Error.Code 'DBSubnetGroupDoesNotCoverEnoughAZs' never fires. Clients that read DBSubnetGroup.VpcId or Subnet.SubnetAvailabilityZone to choose security groups or to assert AZ spread get nothing to read.

Fix: the docdb CreateDBSubnetGroup handler's input validation must resolve each requested subnet id in the account/Region EC2 subnet store, reject unknown ids with InvalidSubnet, collect the distinct AZs, and raise DBSubnetGroupDoesNotCoverEnoughAZs (400, awsQuery code as modeled, message as templated above) when fewer than two distinct AZs are covered while the Region offers two or more — without writing the group to state. The resolved VPC id and per-subnet AZ belong in the docdb subnet-group state record and in its DBSubnetGroup projection (serializer) so VpcId and Subnet.SubnetAvailabilityZone are emitted.

Traces: docdb.subnet-group-rejects-insufficient-az-coverage.

## Evidence
Check docdb.subnet-group-rejects-insufficient-az-coverage (mismatch).
Request: CreateDBSubnetGroup(DBSubnetGroupName=<unique>, DBSubnetGroupDescription='sim2real behavioral check', SubnetIds=[subnet_a, subnet_b]) where subnet_a and subnet_b were both created in the SAME AvailabilityZone of a fresh VPC (10.214.0.0/16), in a Region reporting >= 2 available AZs.
Expected (AWS): HTTP 400, Error.Code 'DBSubnetGroupDoesNotCoverEnoughAZs', Error.Message "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: <az of the requested subnets>. Add subnets to cover at least 2 AZs." and no group left behind.
Actual (emulator): HTTP 200 — AssertionError: expected the call to be rejected, but it succeeded: {'DBSubnetGroup': {'DBSubnetGroupName': ..., 'DBSubnetGroupDescription': 'sim2real behavioral check', 'SubnetGroupStatus': 'Complete', 'Subnets': [{'SubnetIdentifier': 'subnet-...', 'SubnetStatus': 'Active'}, {'SubnetIdentifier': 'subnet-...', 'SubnetStatus': 'Active'}], 'DBSubnetGroupArn': 'arn:aws:rds:<region>:<account>:subgrp:<name>'}}.
Note the actual envelope has no VpcId member and no SubnetAvailabilityZone on either Subnets entry (generated names, ids, request id and timestamp in the captured response are per-run values, not requirements).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. ec2 DescribeAvailabilityZones(Filters=[{state: available}]) and take the first two zone names az0, az1 (the Region must offer >= 2).
3. ec2 CreateVpc(CidrBlock='10.214.0.0/16'); CreateSubnet(10.214.0.0/24, AvailabilityZone=az0) -> subnet_a; CreateSubnet(10.214.1.0/24, AvailabilityZone=az0) -> subnet_b; CreateSubnet(10.214.2.0/24, AvailabilityZone=az1) -> subnet_c.
4. docdb Action=CreateDBSubnetGroup with SubnetIds=[subnet_a, subnet_b] (both in az0). Observed: HTTP 200 with SubnetGroupStatus 'Complete'. Expected: HTTP 400 Error.Code 'DBSubnetGroupDoesNotCoverEnoughAZs' with the AZ-coverage message, and DescribeDBSubnetGroups must not list the name afterwards.
5. Inspect the 200 response: no VpcId member, and no SubnetAvailabilityZone on any Subnets entry.
6. Positive control (must keep working): CreateDBSubnetGroup with SubnetIds=[subnet_a, subnet_c] (az0 + az1) -> 200, SubnetGroupStatus 'Complete', Subnets = {subnet_a, subnet_c}.
