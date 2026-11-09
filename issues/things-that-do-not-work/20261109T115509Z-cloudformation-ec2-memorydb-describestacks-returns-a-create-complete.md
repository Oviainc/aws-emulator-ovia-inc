Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__memorydb.deploy-memorydb-parametergroup-and-subnetgroup-round-trip-then-delete
Issue-Key: cloudformation__ec2__memorydb.deploy-memorydb-parametergroup-and-subnetgroup-round-trip-then-delete|stack_outputs#12|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: c0b75f3dd0ad77b20e3163eb81f4603ab19da1b3

# cloudformation__ec2__memorydb: DescribeStacks returns a CREATE_COMPLETE stack's Outputs entries without OutputValue, so the MemoryDB parameter-group/subnet-group stack's Refs and GetAtt ARNs are unreadable

> _Note: this issue was identified against the code base at commit `c0b75f3dd0ad77b20e3163eb81f4603ab19da1b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Cause 1 of 1: on a stack that the emulator itself reported CREATE_COMPLETE, DescribeStacks serializes Output entries that carry OutputKey but no OutputValue, so any consumer building {OutputKey: OutputValue} raises KeyError: 'OutputValue'.

AWS: Smithy com.amazonaws.cloudformation#Output declares OutputKey / OutputValue / Description / ExportName, and DescribeStacks on a stack in a terminal complete state returns every declared Output with its RESOLVED OutputValue. A stack reported CREATE_COMPLETE never carries an Output whose value failed to resolve — an attribute that cannot be resolved fails the resource (and the stack), it does not yield a valueless Output. Here the Outputs are fed by {"Ref": "Vpc"}, {"Ref": "Subnet"}, {"Ref": "ParameterGroup"}, {"Fn::GetAtt": ["ParameterGroup", "ARN"]}, {"Ref": "SubnetGroup"} and {"Fn::GetAtt": ["SubnetGroup", "ARN"]} over AWS::EC2::VPC, AWS::EC2::Subnet, AWS::MemoryDB::ParameterGroup and AWS::MemoryDB::SubnetGroup. For AWS::MemoryDB::ParameterGroup and AWS::MemoryDB::SubnetGroup, Ref returns the group NAME and GetAtt ARN returns arn:<partition>:memorydb:<region>:<account>:parametergroup/<name> and arn:<partition>:memorydb:<region>:<account>:subnetgroup/<name> respectively — the same ARN MemoryDB's own DescribeParameterGroups / DescribeSubnetGroups reports in its ARN member. The variable parts of those ARNs (partition, region, account, group name) come from the request and the caller's identity; no literal value from any one run is part of the requirement.

Emulator: the stack is observed CREATE_COMPLETE, then at least one Outputs entry is emitted with the value member absent — the key is present, the value is not. Hypothesis: the MemoryDB GetAtt ARN attributes (ParameterGroup.ARN / SubnetGroup.ARN) are never published by their provisioners, and the output serializer emits the key with no value instead of resolving the attribute or failing the resource.

Why: the deploy-then-read pattern is dead for this estate. A client cannot take the parameter-group name, subnet-group name or either ARN out of the stack it just deployed and drive the next MemoryDB call (DescribeParameterGroups, DescribeSubnetGroups, ListTags, CreateCluster) with it, and it is handed a green CREATE_COMPLETE while the outputs it depends on are missing. Any SDK or IaC consumer that indexes Outputs by key crashes rather than seeing a partial result.

Fix: CloudFormation's stack-output resolution and the DescribeStacks output serializer — every Output of a complete stack must carry a resolved OutputValue; that requires the AWS::MemoryDB::ParameterGroup and AWS::MemoryDB::SubnetGroup provisioners to publish their Ref physical id (the group name) and their GetAtt ARN attribute so Fn::GetAtt over them resolves, and an attribute that cannot be resolved must fail the resource rather than yield a valueless Output.

Related but NOT covering: open issue 20261109T113512Z-cloudformation-ec2-network-firewall-describestacks-returns-a-create.md reports the same symptom for the Network Firewall resource set, 20261107T085452Z-cloudformation-8-net-new-behavioral-causes-stack-outputs.md for a nested-stack output, and 20261108T052651Z-cloudformation-3-net-new-causes-the-number-parameter.md for a CDK parent stack. None of them carries this replay key or the AWS::MemoryDB::* resource set.

Traces: cloudformation__ec2__memorydb.deploy-memorydb-parametergroup-and-subnetgroup-round-trip-then-delete.

## Evidence
Expected: after the stack is observed CREATE_COMPLETE, DescribeStacks(StackName=<stack id>)['Stacks'][0]['Outputs'] gives every declared entry (VpcId, SubnetId, ParameterGroupRef, ParameterGroupArn, SubnetGroupRef, SubnetGroupArn) both an OutputKey and a resolved OutputValue — the Ref outputs the group names, the GetAtt outputs arn:<partition>:memorydb:<region>:<account>:parametergroup/<pg-name> and arn:<partition>:memorydb:<region>:<account>:subnetgroup/<sg-name>.

Actual: KeyError: 'OutputValue' raised while building {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])} immediately after the StackStatus assertion for CREATE_COMPLETE passed. The Output entry is present in the list; its value member is absent from the serialized response.

## Steps to Reproduce
Replay the behavioral suite and re-run the check id in trace_ids. Minimal repro: CreateStack with a template declaring AWS::EC2::VPC + AWS::EC2::Subnet (in an available AZ) + AWS::MemoryDB::ParameterGroup (Family memorydb_valkey7) + AWS::MemoryDB::SubnetGroup (SubnetIds: [{"Ref": "Subnet"}]), with Outputs {"VpcId": {"Ref": "Vpc"}, "SubnetId": {"Ref": "Subnet"}, "ParameterGroupRef": {"Ref": "ParameterGroup"}, "ParameterGroupArn": {"Fn::GetAtt": ["ParameterGroup", "ARN"]}, "SubnetGroupRef": {"Ref": "SubnetGroup"}, "SubnetGroupArn": {"Fn::GetAtt": ["SubnetGroup", "ARN"]}}. Poll DescribeStacks(StackName=<StackId>) to CREATE_COMPLETE, then read each Outputs entry's OutputValue.
