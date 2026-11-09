Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__rds.deploy-cfn-rds-db-subnet-group-cross-vpc-subnets-rejected cloudformation__ec2__rds.deploy-cfn-rds-db-subnet-group-provisions-with-getatt-arn-and-lowercased-ref
Issue-Key: cloudformation__ec2__rds.deploy-cfn-rds-db-subnet-group-cross-vpc-subnets-rejected|run#26|AssertionError|assert got in want, f"SubnetGroup CREATE_FAILED reason {got!r}, expected one of {want!r}"
Issue-Key: cloudformation__ec2__rds.deploy-cfn-rds-db-subnet-group-provisions-with-getatt-arn-and-lowercased-ref|run#14|AssertionError|assert outputs["SubnetGroupNameOut"] == lower_name, (
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# cloudformation__ec2__rds: DBSubnetGroup cross-VPC refusal text and resource-status reason diverge, and AWS::RDS::DBSubnetGroup Ref is not lower-cased

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBSubnetGroup

## Description
2 net-new root causes in the CloudFormation/RDS DB-subnet-group path.

Cause 1 of 2: the cross-VPC DBSubnetGroup refusal carries the wrong message, and the CREATE_FAILED ResourceStatusReason is not the SDK-wrapped service message.
AWS: CreateDBSubnetGroup with subnets from two different VPCs is refused 400 InvalidSubnet, and the message names the two VPCs: fixed text 'Subnet has different VPC Id: ' then the offending subnet's VPC id, ' than ', then the group's VPC id (both values come from the request's subnets and EC2 state; either ordering is AWS-legal). Smithy: CreateDBSubnetGroup errors include InvalidSubnet (httpError 400, awsQueryError code InvalidSubnet). When CloudFormation reports that failure it records the service message wrapped by the SDK, i.e. '<service message> (Service: Rds, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)' - the suffix's fixed words are required, the id and count are per-run values.
Emulator: the DBSubnetGroup resource settles CREATE_FAILED with reason ('The requested subnet is invalid, or multiple subnets were requested that are not all in a common VPC.', 'InvalidRequest') - EC2's generic InvalidSubnet prose, naming neither VPC, and with no SDK wrapper at all.
Why: a client that parses the stack-event reason to learn which subnet is in the wrong VPC cannot find the ids, and tooling that strips the '(Service: ...)' wrapper to recover the raw service message sees a message shape AWS never emits.
Fix: the CreateDBSubnetGroup validation/refusal path in the RDS service (its subnet cross-VPC check and message), and the CloudFormation resource-status reason serializer that must wrap a provider refusal in the SDK's '(Service: ..., Status Code: ..., Request ID: ...) (SDK Attempt Count: ...)' form.
Traces: cloudformation__ec2__rds.deploy-cfn-rds-db-subnet-group-cross-vpc-subnets-rejected.

Cause 2 of 2: Ref of an AWS::RDS::DBSubnetGroup returns the template-supplied name verbatim instead of the lower-cased name RDS stored.
AWS: RDS lower-cases a DBSubnetGroupName on creation; the resource's physical id - what Ref returns - is therefore the lower-cased name, and DescribeDBSubnetGroups reads back the lower-cased form. Recorded behavior only for the casing rule; the check supplies a mixed-case name and requires Ref to equal its lower-cased form.
Emulator: Ref returns the mixed-case name exactly as the template wrote it ('Sim2Real-CfnRds-Dbsg-<suffix>') while the resource that exists in RDS is the lower-cased one.
Why: every downstream !Ref consumer (a DBInstance's DBSubnetGroupName, an output a script feeds back into an RDS call) is handed an identifier that does not match the stored group, so the next call 404s on a group the stack just created.
Fix: the AWS::RDS::DBSubnetGroup resource provider's physical-id assignment in the CloudFormation service (it must adopt the identifier RDS actually stored).
Traces: cloudformation__ec2__rds.deploy-cfn-rds-db-subnet-group-provisions-with-getatt-arn-and-lowercased-ref.

## Evidence
Cause 1 - expected one of [('Subnet has different VPC Id: vpc-<a> than vpc-<b> (Service: Rds, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest')]; got ('The requested subnet is invalid, or multiple subnets were requested that are not all in a common VPC.', 'InvalidRequest').
Cause 2 - expected Ref 'sim2real-cfnrds-dbsg-<suffix>' (lower-cased); got 'Sim2Real-CfnRds-Dbsg-<suffix>'.

## Steps to Reproduce
Cause 1: deploy a stack whose AWS::RDS::DBSubnetGroup lists two subnets from two different VPCs; read the DBSubnetGroup CREATE_FAILED event's ResourceStatusReason.
Cause 2: deploy a stack whose AWS::RDS::DBSubnetGroup has a mixed-case DBSubnetGroupName and output !Ref of it; compare the output with DescribeDBSubnetGroups.
