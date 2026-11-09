Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__network-firewall.deploy-cfn-nfwr-firewall-and-logging-round-trip-and-delete
Issue-Key: cloudformation__ec2__network-firewall.deploy-cfn-nfwr-firewall-and-logging-round-trip-and-delete|stack_outputs#13|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# cloudformation__ec2__network-firewall: DescribeStacks returns a CREATE_COMPLETE stack's Outputs entries without OutputValue, so the Network Firewall stack's Refs and GetAtts are unreadable

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Cause 1 of 1: on a stack that reached CREATE_COMPLETE, DescribeStacks serializes Output entries that carry OutputKey but no OutputValue.
AWS: Smithy com.amazonaws.cloudformation#Output declares OutputKey/OutputValue/Description/ExportName, and DescribeStacks on a stack in a terminal complete state returns every declared Output with its RESOLVED OutputValue. Here the outputs are fed by {"Ref": "Subnet"}, {"Ref": "Policy"}, {"Ref": "Firewall"}, {"Fn::GetAtt": ["Firewall", "FirewallId"]}, {"Fn::Join": [",", {"Fn::GetAtt": ["Firewall", "EndpointIds"]}]} and {"Ref": "Logging"} over AWS::EC2::VPC/Subnet, AWS::NetworkFirewall::FirewallPolicy, AWS::NetworkFirewall::Firewall, AWS::Logs::LogGroup and AWS::NetworkFirewall::LoggingConfiguration — for AWS::NetworkFirewall::Firewall, Ref returns the firewall ARN, GetAtt FirewallId the firewall id and GetAtt EndpointIds a list of '<availability zone>:<vpce id>' entries; for AWS::NetworkFirewall::LoggingConfiguration, Ref returns the firewall ARN. A stack reported CREATE_COMPLETE never has an Output whose value failed to resolve: if an attribute cannot be resolved the resource/stack fails instead.
Emulator: the stack is reported CREATE_COMPLETE, but at least one Outputs entry is emitted with the value member absent, so any consumer building {OutputKey: OutputValue} raises KeyError: 'OutputValue'. Hypothesis: the unresolvable attributes are the Network Firewall ones (GetAtt EndpointIds / FirewallId, or the LoggingConfiguration Ref) and the serializer emits the key with an empty value rather than failing the stack or resolving the attribute.
Why: the deploy-then-read pattern is dead for this estate — a client cannot take the firewall ARN, firewall id, endpoint ids, policy ARN or subnet id out of the stack it just deployed and drive the next Network Firewall / EC2 call with it, and it is handed a green CREATE_COMPLETE while the outputs it depends on are missing.
Fix: CloudFormation's stack-output resolution and the DescribeStacks serializer — every Output of a complete stack must carry a resolved OutputValue; that requires the AWS::NetworkFirewall::Firewall and ::LoggingConfiguration provisioners to publish their Ref physical id and their GetAtt attributes (FirewallId, EndpointIds) so Fn::GetAtt/Fn::Join over them resolve, and an attribute that cannot be resolved must fail the resource rather than yield a valueless Output. Related but not covering: open issue 20261107T085452Z-cloudformation-8-net-new-behavioral-causes-stack-outputs.md describes the same symptom as its Cause 1 for a nested-stack output, and 20261108T052651Z-cloudformation-3-net-new-causes-the-number-parameter.md for a CDK parent stack; neither carries this replay key or this resource set.
Traces: cloudformation__ec2__network-firewall.deploy-cfn-nfwr-firewall-and-logging-round-trip-and-delete.

## Evidence
Expected: DescribeStacks(StackName=<stack id>)['Stacks'][0]['Outputs'] gives every entry both OutputKey and a resolved OutputValue (SubnetId, PolicyArn, FirewallArn, FirewallId, FirewallEndpointIds, LoggingRef).
Actual: KeyError: 'OutputValue' raised while building {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])} immediately after the stack was observed CREATE_COMPLETE — the Output entry is present, the value member is absent.

## Steps to Reproduce
Replay the behavioral suite and re-run the check id in trace_ids. Minimal repro: CreateStack with a template declaring AWS::EC2::VPC + AWS::EC2::Subnet + AWS::NetworkFirewall::FirewallPolicy + AWS::NetworkFirewall::Firewall (SubnetMappings referencing the subnet) + AWS::Logs::LogGroup + AWS::NetworkFirewall::LoggingConfiguration, with Outputs {"SubnetId": {"Ref": "Subnet"}, "PolicyArn": {"Ref": "Policy"}, "FirewallArn": {"Ref": "Firewall"}, "FirewallId": {"Fn::GetAtt": ["Firewall", "FirewallId"]}, "FirewallEndpointIds": {"Fn::Join": [",", {"Fn::GetAtt": ["Firewall", "EndpointIds"]}]}, "LoggingRef": {"Ref": "Logging"}}; wait for CREATE_COMPLETE and read each Outputs entry's OutputValue.
