Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appstream__ec2.e2e-appstream-stack-with-streaming-vpce-lifecycle
Issue-Key: appstream__ec2.e2e-appstream-stack-with-streaming-vpce-lifecycle|phase#7|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: ef6e0f5e66c899ede0df1e04050c78d39daac904

# appstream__ec2: EC2 CreateVpcEndpoint leaves PrivateDnsEnabled false for an Interface endpoint when the request omits the flag, instead of applying AWS's true default (1 cause)

> _Note: this issue was identified against the code base at commit `ef6e0f5e66c899ede0df1e04050c78d39daac904`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
One net-new root cause, in the EC2 CreateVpcEndpoint handler reached by the AppStream streaming-endpoint lifecycle check.

Cause 1 of 1: an Interface VPC endpoint created WITHOUT PrivateDnsEnabled in the request is reported with PrivateDnsEnabled=false, so the private hosted zone is never associated and the endpoint claims none of the service's advertised private DNS names.
AWS: recorded behavior only for the default value — Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) declares CreateVpcEndpointRequest.PrivateDnsEnabled as a plain optional Boolean with no @default trait; its documentation says "(Interface endpoint) Indicates whether to associate a private hosted zone with the specified VPC. The private hosted zone contains a record set for the default public DNS name for the service for the Region ... To use a private hosted zone, you must set the following VPC attributes to true: enableDnsHostnames and enableDnsSupport." The recording shows that when the flag is omitted on an Interface endpoint in a VPC with both DNS attributes true, AWS applies its service-side default of TRUE: DescribeVpcEndpoints reports PrivateDnsEnabled=true once the endpoint is available, and its DnsEntries include every private name DescribeVpcEndpointServices advertises for that service under PrivateDnsNames.
Emulator: DescribeVpcEndpoints reports PrivateDnsEnabled=False for exactly that request (replay detail: "the reference solution never sets the flag, so the AWS-service default (true) must apply; got False"), i.e. the handler stores the absent boolean as false rather than defaulting it for the Interface endpoint type.
Why: private DNS is the mechanism that makes an interface endpoint usable — clients reach the service at its normal regional hostname and resolve to the endpoint's ENIs. With the flag false the endpoint is only reachable through its generated endpoint-specific names, so every client that was written against AWS's default (no flag in the template or CLI call, then traffic over the standard hostname) silently fails to use the endpoint, and any check or policy that asserts private DNS is on for a private-connectivity endpoint reads the opposite of AWS. It also means the endpoint's DnsEntries do not contain the service's advertised private names, so cross-referencing endpoint to service details finds no match.
Fix: the CreateVpcEndpoint handler in services/service-com-amazonaws-ec2-amazonec2 — when VpcEndpointType is Interface and PrivateDnsEnabled is absent from the request, default it to true and record the resulting private DNS entries, so the DescribeVpcEndpoints projection reports true and lists the service's private DNS names.
Traces: appstream__ec2.e2e-appstream-stack-with-streaming-vpce-lifecycle (phase 2/4 'solve').

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the booted emulator; per-service rate appstream__ec2 6/17 (35%).

Check appstream__ec2.e2e-appstream-stack-with-streaming-vpce-lifecycle [mismatch], phase 2/4 'solve':
  sequence (graded, frozen): ec2 DescribeVpcs, DescribeSubnets, DescribeSecurityGroups, CreateVpcEndpoint, appstream CreateStack
  call: ec2.create_vpc_endpoint(VpcId=<deployed vpc>, ServiceName='com.amazonaws.<region>.appstream.streaming', VpcEndpointType='Interface', SubnetIds=[<deployed subnet>], SecurityGroupIds=[<vpc default sg>])  — PrivateDnsEnabled NOT supplied
  the preceding assertions passed: the endpoint reaches State='available', and (VpcEndpointType, ServiceName, VpcId, SubnetIds, Groups) all match the request
  expected: DescribeVpcEndpoints -> PrivateDnsEnabled is True, and DnsEntries contain every PrivateDnsName advertised by DescribeVpcEndpointServices for that ServiceName
  got:      AssertionError: [phase 2/4 solve] the reference solution never sets the flag, so the AWS-service default (true) must apply; got False

The VPC in the recording has enableDnsSupport and enableDnsHostnames both true (asserted and passing in the preceding 'deploy' phase), which is the precondition the model's documentation names for a private hosted zone. Endpoint ids, vpc/subnet/sg ids and the region are per-run values and are not part of the requirement.

Smithy evidence: CreateVpcEndpointRequest.PrivateDnsEnabled is an optional Boolean with no @default; the default-true behavior for Interface endpoints is grounded in the recording and in the member's documentation of the private-hosted-zone association.

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from it.
2. Run the behavioral suite (simparity-behavioral-aws-v1) and replay check appstream__ec2.e2e-appstream-stack-with-streaming-vpce-lifecycle.
3. Equivalently by hand: create a VPC with enableDnsSupport and enableDnsHostnames set to true plus one subnet; then call ec2 CreateVpcEndpoint with VpcEndpointType='Interface', ServiceName='com.amazonaws.<region>.appstream.streaming', the subnet and the VPC's default security group, and WITHOUT PrivateDnsEnabled.
4. Poll DescribeVpcEndpoints until State='available' and read PrivateDnsEnabled: it is false, where AWS reports true.
5. Compare the endpoint's DnsEntries against DescribeVpcEndpointServices(ServiceNames=[that service]).ServiceDetails[0].PrivateDnsNames: none of the advertised private names are claimed.
