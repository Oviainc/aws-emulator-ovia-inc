Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.modify-cluster-parameter-invalid-value-rejected rds.subnet-group-vpc-and-arn-round-trip
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.modify-cluster-parameter-invalid-value-rejected|_client_error#11|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: rds.subnet-group-vpc-and-arn-round-trip|run#7|AssertionError|assert created["VpcId"] == vpc_id, (
Issue-Commit: 8ec1c361db968173953f5aad3d85a175bc9ad54a

# rds: 2 net-new root causes — ModifyDBClusterParameterGroup accepts a ParameterValue outside the parameter's AllowedValues, and CreateDBSubnetGroup reports a fabricated VpcId instead of the VPC owning the supplied subnets

> _Note: this issue was identified against the code base at commit `8ec1c361db968173953f5aad3d85a175bc9ad54a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBClusterParameterGroup

## Description
Two independent RDS control-plane defects surfaced by the behavioral suite. 2 causes; each must be closed and re-replayed separately.

Cause 1 of 2: ModifyDBClusterParameterGroup silently accepts a ParameterValue that is not a member of the parameter's AllowedValues.
AWS: Smithy (com.amazonaws.rds#Parameter) models each parameter's DataType ("Specifies the valid data type for the parameter"), AllowedValues ("Specifies the valid range of values for the parameter") and IsModifiable, and com.amazonaws.rds#ModifyDBClusterParameterGroupMessage takes Parameters: ParametersList. ModifyDBClusterParameterGroup's own modeled error list carries only DBParameterGroupNotFoundFault (404) and InvalidDBParameterGroupStateFault (400); the refusal for a value outside a parameter's own domain is the query-protocol common error InvalidParameterValue, HTTP 400, Type Sender. Recorded behavior: a request setting a boolean, modifiable parameter of an aurora-postgresql family group to a value that cannot be a member of its AllowedValues is refused with Code InvalidParameterValue and Message "Invalid boolean value: <ParameterValue>" — fixed text "Invalid boolean value: ", the variable part being the ParameterValue echoed from the request — and the parameter keeps its engine-default value, so a subsequent DescribeDBClusterParameters still reports Source other than "user".
Emulator: answers HTTP 200 with a DBClusterParameterGroupNameMessage echoing DBClusterParameterGroupName, with no error at all, and stores the engine-incompatible value.
Why: a client that relies on RDS to validate parameter domains gets a parameter group it believes is valid but which holds a value no engine will accept; the failure then surfaces only when the group is attached to a cluster (or never, against the emulator). Negative tests that assert InvalidParameterValue pass against AWS and fail here, and the stored override corrupts later DescribeDBClusterParameters reads (Source becomes "user" for a value AWS never accepted).
Fix: the ModifyDBClusterParameterGroup handler's per-parameter value validation (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/modify_db_cluster_parameter_group.rs) and the engine-default parameter catalog it must validate against (DataType/AllowedValues/IsModifiable per parameter), rejecting before any state write so the effect stays absent. Related open issue that does NOT carry this check's replay key: 20261029T214632Z-rds-accepts-nine-requests-aws-refuses-missing-request.md — its Cause 2 covers an UNKNOWN ParameterName on the same operation, not a known parameter with an out-of-domain VALUE, and its Cause 9 covers IsModifiable on ModifyDBParameterGroup.
Traces: rds.modify-cluster-parameter-invalid-value-rejected.

Cause 2 of 2: CreateDBSubnetGroup returns a VpcId it minted itself instead of the VPC that owns the subnets named in the request.
AWS: Smithy (com.amazonaws.rds#DBSubnetGroup) models VpcId as "Provides the VpcId of the DB subnet group", with Subnets: SubnetList of com.amazonaws.rds#Subnet{SubnetIdentifier, SubnetAvailabilityZone, SubnetStatus}; CreateDBSubnetGroup input requires SubnetIds (SubnetIdentifierList). AWS resolves VpcId from those subnet ids — it is the id of the VPC that owns them — and the value is stable: the same VpcId and DBSubnetGroupArn come back byte-identical from the CreateDBSubnetGroup response and from every later DescribeDBSubnetGroups read, never recomputed or regenerated per read.
Emulator: answers 200 but reports a VpcId unrelated to the supplied subnets — a freshly generated id (in this replay the subnets belonged to the EC2-created VPC the check had just made, and CreateDBSubnetGroup answered with a different, handler-minted id; the literal ids are per-run values, the requirement is that VpcId equals the VPC owning the named subnets).
Why: the VPC of a DB subnet group is a placement precondition — it is what a client uses to pick security groups, route tables and the VPC a DB instance created with that group will land in. A client reading the create response back cannot verify it used the right VPC, and cross-checking RDS against EC2 (DescribeSubnets/DescribeVpcs) always disagrees, so VPC-scoped wiring built on the group is wrong.
Fix: the CreateDBSubnetGroup handler (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_subnet_group.rs) and the subnet-group record it writes to state: resolve each supplied subnet id through the EC2 state and take the group's VpcId from the VPC that owns them, instead of generating one, and persist it so repeated describes return the identical value. Related open issue that does NOT carry this check's replay key: 20261102T002746Z-rds-describeenginedefaultclusterparameters-and-createdbsubnetgroup-synthesize-data-the-request.md (its Cause 2 reports the same fabrication from the wire corpus, keyed on SubnetAvailabilityZone.Name); see also 20261029T214632Z-rds-omits-vpc-subnet-and-availability-zone-attributes.md and 20261030T001542Z-rds-db-subnet-group-and-db-instance-projections.md.
Traces: rds.subnet-group-vpc-and-arn-round-trip.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (84/543 checks passed; the emulator was serving — rds answered 77 checks).

Cause 1 — check rds.modify-cluster-parameter-invalid-value-rejected [mismatch]:
  The check creates a DB cluster parameter group in the current default aurora-postgresql DBParameterGroupFamily, discovers a boolean, modifiable, value-resolvable parameter in that family, and calls ModifyDBClusterParameterGroup with ParameterValue="sim2real-not-a-valid-value-xyz", ApplyMethod=pending-reboot.
  Expected: ClientError, Code "InvalidParameterValue", Message "Invalid boolean value: sim2real-not-a-valid-value-xyz" (HTTP 400), and the parameter still reporting Source != "user".
  Actual: AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'DBClusterParameterGroupName': 'sim2real-rds-cpg-e663326d73a1', 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'text/xml', ...}}}.
  (The group name and request id above are this run's values; the requirement is the 400/InvalidParameterValue refusal whose fixed text is "Invalid boolean value: " followed by the ParameterValue from the request, and the absence of any stored override.)

Cause 2 — check rds.subnet-group-vpc-and-arn-round-trip [mismatch]:
  The check creates a VPC and subnets through EC2, then CreateDBSubnetGroup with those SubnetIds.
  Expected: DBSubnetGroup.VpcId equal to the VPC owning the supplied subnets, and VpcId/DBSubnetGroupArn byte-identical on repeated DescribeDBSubnetGroups.
  Actual: AssertionError: VpcId on create: expected 'vpc-00000000000000024', got 'vpc-cd3f34b29191d275e'.
  (Both ids are per-run values; the requirement is VpcId == the VPC of the named subnets. The check failed on the first assertion, so the ARN-stability leg was never reached.)
  Model: com.amazonaws.rds#DBSubnetGroup{VpcId, Subnets: SubnetList, DBSubnetGroupArn}; com.amazonaws.rds#CreateDBSubnetGroupMessage{SubnetIds required}.

## Steps to Reproduce
1. git_clone the repo and build/boot the emulator from this checkout.
2. Replay the behavioral suite (simparity-behavioral-aws-v1) and re-run exactly the checks in trace_ids.
3. Cause 1 — rds.modify-cluster-parameter-invalid-value-rejected. By hand: aws rds describe-db-engine-versions --engine aurora-postgresql --default-only to get the DBParameterGroupFamily; aws rds create-db-cluster-parameter-group --db-cluster-parameter-group-name <pg> --db-parameter-group-family <family> --description d; pick a boolean, modifiable parameter P from aws rds describe-db-cluster-parameters --db-cluster-parameter-group-name <pg>; then aws rds modify-db-cluster-parameter-group --db-cluster-parameter-group-name <pg> --parameters ParameterName=P,ParameterValue=sim2real-not-a-valid-value-xyz,ApplyMethod=pending-reboot. The emulator answers 200; AWS answers 400 InvalidParameterValue "Invalid boolean value: sim2real-not-a-valid-value-xyz". Then aws rds describe-db-cluster-parameters --db-cluster-parameter-group-name <pg> and observe P now reports Source=user with the bogus value.
4. Cause 2 — rds.subnet-group-vpc-and-arn-round-trip. By hand: aws ec2 create-vpc, aws ec2 create-subnet twice in two Availability Zones of that VPC, then aws rds create-db-subnet-group --db-subnet-group-name <name> --db-subnet-group-description d --subnet-ids <subnet-a> <subnet-b> and compare DBSubnetGroup.VpcId with the VpcId returned by create-vpc; then aws rds describe-db-subnet-groups --db-subnet-group-name <name> twice and compare VpcId and DBSubnetGroupArn for byte-identity.
