Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-asgeni-traj-launch-template-ni-accepted-then-refused-at-launch
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-asgeni-traj-launch-template-ni-accepted-then-refused-at-launch ec2-asgeni-traj-run-instances-ni-device-index-and-placement ec2-asgeni-traj-run-instances-ni-param-conflicts ec2-ipam-trajwire-create-vpc-parameter-mistakes ec2-ipam-trajwire-stale-pool-reference ec2-ipam-trajwire-vpc-from-top-pool-then-child ec2-pip-traj-ipv6-on-ipv4-only-subnet ec2-sshp-traj-eip-association-needs-an-attached-igw ec2-vpce-traj-iot-data-endpoint-create-refusals
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateNetworkInterface|4xx->5xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateNetworkInterface|4xx->5xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateNetworkInterface|4xx->5xx|status
Issue-Key: ec2|AssociateAddress|4xx->5xx|$.Response.Errors.Error.Code
Issue-Key: ec2|AssociateAddress|4xx->5xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AssociateAddress|4xx->5xx|status
Issue-Key: ec2|CreateVpcEndpoint|4xx->5xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateVpcEndpoint|4xx->5xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateVpcEndpoint|4xx->5xx|status
Issue-Related: 20261103T071133Z-ec2-dryrun-is-never-honoured-dry-run-probes.md
Issue-Commit: ce4f28f15b4841beefbad6c596b9d2158570753e

# EC2 answers 501 NotImplemented on implemented operations for request variants AWS serves or refuses

> _Note: this issue was identified against the code base at commit `ce4f28f15b4841beefbad6c596b9d2158570753e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Five request variants on operations the emulator does implement are answered HTTP 501 with Code 'NotImplemented' and a message that names the emulator ('operation <X> with <variant> is scaffolded but not implemented in this emulator'). AWS answers each of these with a modeled 400 refusal. 501/NotImplemented is not in EC2's error vocabulary, so no SDK retry or error-mapping path handles it; the caller sees a transport-level server error. (Related open issue 20261030T000411Z-ec2-refuses-valid-requests-on-implemented-operations-with describes the same family, but these five clusters carry no replay key and are unowned.)

Cause 1 of 5: RunInstances with a LaunchTemplate plus instance-level security groups.
AWS: recorded behavior only — 400 Code 'InvalidParameterCombination', Message 'Network interfaces and an instance-level security groups may not be specified on the same request'.
Emulator: 501 'NotImplemented' / 'operation RunInstances with a LaunchTemplate is scaffolded but not implemented in this emulator'.
Why: the whole launch-template launch path is unusable, and the authoring mistake the caller made is never reported.
Fix: the launch-template branch of services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs (resolve template version data, then apply AWS's parameter-combination validation).
Traces: ec2-asgeni-traj-launch-template-ni-accepted-then-refused-at-launch#task#6 (cluster also covers task#9/#10/#13 and ec2-asgeni-traj-run-instances-ni-device-index-and-placement#task#4..#10, ec2-asgeni-traj-run-instances-ni-param-conflicts#task#5..#8 — 15 steps).

Cause 2 of 5: CreateVpc with an IPAM-allocated CIDR.
AWS: recorded behavior only — 400 Code 'InvalidParameterCombination', Message 'The request cannot contain empty CIDR and Netmask Length for a pool with no default allocation netmask length set. Exactly one of them should be provided.'
Emulator: 501 'NotImplemented' / 'operation CreateVpc with an IPAM-allocated CIDR block is scaffolded but not implemented in this emulator'.
Why: IPAM-driven VPC creation cannot be exercised, and the parameter rule is never enforced.
Fix: the Ipv4IpamPoolId branch of create_vpc.rs.
Traces: ec2-ipam-trajwire-create-vpc-parameter-mistakes#task#11 (cluster also covers task#12/#13/#14, ec2-ipam-trajwire-stale-pool-reference#task#4/#5/#6, ec2-ipam-trajwire-vpc-from-top-pool-then-child#task#7/#11).

Cause 3 of 5: CreateNetworkInterface with IPv6 addresses on an IPv4-only subnet.
AWS: recorded behavior only — 400 Code 'InvalidParameterValue', Message 'Subnet does not contain any IPv6 CIDR block ranges'.
Emulator: 501 'NotImplemented' / 'operation CreateNetworkInterface with IPv6 addresses is scaffolded but not implemented in this emulator'.
Why: the IPv6 branch is unusable and the subnet precondition is never checked.
Fix: the IPv6 request branch of create_network_interface.rs.
Traces: ec2-pip-traj-ipv6-on-ipv4-only-subnet#task#7.

Cause 4 of 5: AssociateAddress with a networkInterfaceId.
AWS: recorded behavior only — 400 Code 'Gateway.NotAttached', Message 'Network vpc-00000000000000024 is not attached to any internet gateway' (fixed text with the VPC id from state substituted).
Emulator: 501 'NotImplemented' / 'operation AssociateAddress with a networkInterfaceId is scaffolded but not implemented in this emulator'.
Why: EIP association by ENI — the normal path for a secondary interface — cannot be used, and the Gateway.NotAttached precondition disappears.
Fix: the network-interface branch of associate_address.rs.
Traces: ec2-sshp-traj-eip-association-needs-an-attached-igw#task#6.

Cause 5 of 5: CreateVpcEndpoint with IpAddressType=ipv6.
AWS: recorded behavior only — 400 Code 'InvalidSubnet', Message 'Subnet subnet-0000000000000002b is not compatible with ipv6 addressType.' (fixed text with the request's subnet id and address type substituted).
Emulator: 501 'NotImplemented' / 'operation CreateVpcEndpoint with the ipv6 IP address type is scaffolded but not implemented in this emulator'.
Why/Fix: as above, in create_vpc_endpoint.rs (IpAddressType branch).
Traces: ec2-vpce-traj-iot-data-endpoint-create-refusals#task#7.

## Evidence
Replayed wire corpus simparity-wire-aws-v1. Representative: ec2-asgeni-traj-launch-template-ni-accepted-then-refused-at-launch task#6 — expected 400 / 'InvalidParameterCombination' / 'Network interfaces and an instance-level security groups may not be specified on the same request'; actual 501 / 'NotImplemented' / 'operation RunInstances with a LaunchTemplate is scaffolded but not implemented in this emulator' (15 steps in that cluster alone). The same 501 substitution appears for CreateVpc with an IPAM CIDR (expected 400 InvalidParameterCombination), CreateNetworkInterface with IPv6 (expected 400 InvalidParameterValue 'Subnet does not contain any IPv6 CIDR block ranges'), AssociateAddress by ENI (expected 400 Gateway.NotAttached) and CreateVpcEndpoint with ipv6 (expected 400 InvalidSubnet).

## Steps to Reproduce
1. Replay fixture ec2-asgeni-traj-launch-template-ni-accepted-then-refused-at-launch step task#6 (RunInstances referencing a launch template) and observe HTTP 501 NotImplemented.
2. Replay each fixture#step in trace_ids: CreateVpc with Ipv4IpamPoolId, CreateNetworkInterface with Ipv6AddressCount, AssociateAddress with NetworkInterfaceId, CreateVpcEndpoint with IpAddressType=ipv6.
3. In each case compare against the recorded 400 code and message.
