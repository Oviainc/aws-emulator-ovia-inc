Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-delete-local-gateway-virtual-interface ec2.op-contract-modify-vpc-endpoint-payer-responsibility ec2.op-contract-modify-vpc-endpoint-service-configuration ec2.op-contract-provision-public-ipv4-pool-cidr-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-delete-local-gateway-virtual-interface|run#6|AssertionError|assert dry_code == "DryRunOperation", f"expected DryRunOperation, got {dry_code}"
Issue-Key: ec2.op-contract-modify-vpc-endpoint-payer-responsibility|run#11|AssertionError|assert err == "DryRunOperation", err
Issue-Key: ec2.op-contract-modify-vpc-endpoint-service-configuration|run#6|AssertionError|assert err == "DryRunOperation", err
Issue-Key: ec2.op-contract-provision-public-ipv4-pool-cidr-rejected|run#11|AssertionError|assert err == "InvalidPublicIpv4Pool.NotFound", f"expected InvalidPublicIpv4Pool.NotFound, got {err!r}"
Issue-Commit: 6bf1871c82f616f42fdbabf9cffce393eeefa7cd

# EC2: 4 more operations are unimplemented scaffold stubs answering 501 NotImplemented (DeleteLocalGatewayVirtualInterface, ModifyVpcEndpointPayerResponsibility, ModifyVpcEndpointServiceConfiguration, ProvisionPublicIpv4PoolCidr)

> _Note: this issue was identified against the code base at commit `6bf1871c82f616f42fdbabf9cffce393eeefa7cd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyVpcEndpointServiceConfiguration

## Description
Four distinct net-new root causes: four EC2 operations whose handler is still the generated scaffold, so every call — DryRun probe, refusal path and success path alike — answers HTTP 501 with code NotImplemented. Each is its own work item and all four must be closed before this issue is submitted. The similar open issue 20261029T175114Z-ec2-dryrun-contract-authorized-dryrun-requests-answer-notimplemented.md describes the same DryRun-on-a-scaffold shape, but it does not carry these checks' replay keys and names none of these four operations, so it does not cover them.

Cause 1 of 4: DeleteLocalGatewayVirtualInterface answers NotImplemented for both its DryRun probe and its real not-found refusal.
AWS: Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) models DeleteLocalGatewayVirtualInterface with required LocalGatewayVirtualInterfaceId and a DryRun Boolean whose documentation states the authorized response is DryRunOperation. Recorded behavior: DryRun=true on an id that was never created answers HTTP 412, code DryRunOperation, message 'Request would have succeeded, but DryRun flag is set.' (entirely fixed text) — the permission check precedes existence validation; the same call without DryRun answers code InvalidLocalGatewayVirtualInterfaceID.NotFound with message "The local-gateway-virtual-interface ID '<lgw-vif-id>' does not exist" (fixed text; the id is the one from the request).
Emulator: both calls answer code NotImplemented; services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_local_gateway_virtual_interface.rs still carries the 'SCAFFOLDED, NOT IMPLEMENTED' marker and returns Scaffolded::not_implemented.
Why: a client cannot probe permissions, and cannot distinguish 'this id does not exist' from 'the service cannot do this', so Outposts local-gateway teardown and preflight paths abort on an un-modeled error.
Fix: implement the operation in its handler file named above (DryRun gate first, then id resolution and the not-found refusal) and drop the scaffold marker.
Traces: ec2.op-contract-delete-local-gateway-virtual-interface.

Cause 2 of 4: ModifyVpcEndpointPayerResponsibility is a scaffold, so even its DryRun probe answers NotImplemented.
AWS: Smithy models ModifyVpcEndpointPayerResponsibility(DryRun, ServiceId, required VpcEndpointId, required PayerResponsibility, required Scope) -> {VpcEndpointId, PayerResponsibilities}. Recorded behavior: DryRun=true on a valid request answers HTTP 412, code DryRunOperation, message 'Request would have succeeded, but DryRun flag is set.' and mutates nothing; a real call applies the setting and echoes it in PayerResponsibilities (Scope + PayerResponsibilityType), which DescribeVpcEndpoints then reports for that endpoint.
Emulator: code NotImplemented; services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_vpc_endpoint_payer_responsibility.rs is the scaffold stub.
Why: no client can move endpoint billing responsibility or even preflight it, and the endpoint's PayerResponsibilities view never changes.
Fix: implement that handler file (DryRun short-circuit, then write the payer-responsibility entry into the endpoint's state and echo it; DescribeVpcEndpoints must read it back).
Traces: ec2.op-contract-modify-vpc-endpoint-payer-responsibility.

Cause 3 of 4: ModifyVpcEndpointServiceConfiguration is a scaffold, so AcceptanceRequired can never be changed.
AWS: Smithy models ModifyVpcEndpointServiceConfiguration(DryRun, required ServiceId, AcceptanceRequired, Add/Remove NetworkLoadBalancerArns, ...) -> {Return: Boolean}. Recorded behavior: DryRun=true answers HTTP 412, code DryRunOperation, message 'Request would have succeeded, but DryRun flag is set.' and leaves the configuration untouched; a real call returns Return=true and the new AcceptanceRequired value becomes visible through DescribeVpcEndpointServiceConfigurations.
Emulator: code NotImplemented; services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_vpc_endpoint_service_configuration.rs is the scaffold stub.
Why: an endpoint service can be created but never reconfigured, so acceptance-required flows (and the NLB add/remove paths) are unreachable and preflight fails hard.
Fix: implement that handler file (DryRun gate, mutate the service configuration in state, answer Return=true, and make the change readable via DescribeVpcEndpointServiceConfigurations).
Traces: ec2.op-contract-modify-vpc-endpoint-service-configuration.

Cause 4 of 4: ProvisionPublicIpv4PoolCidr is a scaffold, so an unknown public IPv4 pool id is answered NotImplemented instead of refused.
AWS: Smithy models ProvisionPublicIpv4PoolCidr(DryRun, required IpamPoolId, required PoolId: Ipv4PoolEc2Id, required NetmaskLength, NetworkBorderGroup) -> {PoolId, PoolAddressRange}. Recorded behavior: a request naming a PoolId that does not exist (against a real create-complete BYOIP IPAM pool) is refused with code InvalidPublicIpv4Pool.NotFound and message "The pool ID '<ipv4pool-ec2-id>' does not exist." (fixed text; the pool id is the request's value), and the targeted IPAM pool is left with no provisioned CIDR (GetIpamPoolCidrs stays empty).
Emulator: code NotImplemented; services/service-com-amazonaws-ec2-amazonec2/src/ops/provision_public_ipv4_pool_cidr.rs is the scaffold stub.
Why: BYOIP provisioning into an IPAM pool is unavailable, and a client cannot tell a bad pool id from an unimplemented service, so error-driven cleanup/branching misfires.
Fix: implement that handler file (resolve PoolId and IpamPoolId, refuse unknown ids with the not-found code and message above, provision on success without mutating state on refusal).
Traces: ec2.op-contract-provision-public-ipv4-pool-cidr-rejected.

## Evidence
Cause 1 — ec2.op-contract-delete-local-gateway-virtual-interface [mismatch]: `AssertionError: expected DryRunOperation, got NotImplemented` on DeleteLocalGatewayVirtualInterface(LocalGatewayVirtualInterfaceId='lgw-vif-00000000000000000', DryRun=True). The check's next assertions expect code InvalidLocalGatewayVirtualInterfaceID.NotFound with message "The local-gateway-virtual-interface ID '<id>' does not exist" for the non-DryRun call. Handler file still marked 'SCAFFOLDED, NOT IMPLEMENTED' (src/ops/delete_local_gateway_virtual_interface.rs:4).

Cause 2 — ec2.op-contract-modify-vpc-endpoint-payer-responsibility [mismatch]: `AssertionError: NotImplemented` at `assert err == "DryRunOperation"` after creating an NLB-backed endpoint service and an interface endpoint; expected HTTP 412 / DryRunOperation / 'Request would have succeeded, but DryRun flag is set.' Handler file marked 'SCAFFOLDED, NOT IMPLEMENTED' (src/ops/modify_vpc_endpoint_payer_responsibility.rs:4, returns Scaffolded::not_implemented at line 157).

Cause 3 — ec2.op-contract-modify-vpc-endpoint-service-configuration [mismatch]: `AssertionError: NotImplemented` at `assert err == "DryRunOperation"` for ModifyVpcEndpointServiceConfiguration(ServiceId=..., AcceptanceRequired=True, DryRun=True); the success path then expects Return=true and AcceptanceRequired=True via DescribeVpcEndpointServiceConfigurations. Handler file marked 'SCAFFOLDED, NOT IMPLEMENTED' (src/ops/modify_vpc_endpoint_service_configuration.rs:4).

Cause 4 — ec2.op-contract-provision-public-ipv4-pool-cidr-rejected [mismatch]: `AssertionError: expected InvalidPublicIpv4Pool.NotFound, got 'NotImplemented'` for ProvisionPublicIpv4PoolCidr(IpamPoolId=<real create-complete byoip pool>, PoolId='ipv4pool-ec2-00000000000000000', NetmaskLength=24); expected message "The pool ID 'ipv4pool-ec2-00000000000000000' does not exist." (the id is the request's value). Handler file marked 'SCAFFOLDED, NOT IMPLEMENTED' (src/ops/provision_public_ipv4_pool_cidr.rs:4).

## Steps to Reproduce
1. Boot the emulator and point an EC2 client at it with credentials it accepts.
2. Cause 1: call DeleteLocalGatewayVirtualInterface(LocalGatewayVirtualInterfaceId='lgw-vif-00000000000000000', DryRun=True), then the same call without DryRun. Observe NotImplemented for both instead of DryRunOperation (HTTP 412) and InvalidLocalGatewayVirtualInterfaceID.NotFound.
3. Cause 2: create a VPC + subnet + internal NLB, a VPC endpoint service configuration, and an interface VPC endpoint; call ModifyVpcEndpointPayerResponsibility(VpcEndpointId=..., ServiceId=..., PayerResponsibility='vpc-endpoint-service-account', Scope='vpc-endpoint-charges', DryRun=True) and then without DryRun. Observe NotImplemented instead of DryRunOperation and then a 200 echoing PayerResponsibilities.
4. Cause 3: with the same endpoint service, call ModifyVpcEndpointServiceConfiguration(ServiceId=..., AcceptanceRequired=True, DryRun=True) and then without DryRun. Observe NotImplemented instead of DryRunOperation and then Return=true with AcceptanceRequired=True visible in DescribeVpcEndpointServiceConfigurations.
5. Cause 4: CreateIpam, CreateIpamPool in the public default scope with PublicIpSource='byoip', wait for create-complete, then call ProvisionPublicIpv4PoolCidr(IpamPoolId=<pool>, PoolId='ipv4pool-ec2-00000000000000000', NetmaskLength=24). Observe NotImplemented instead of InvalidPublicIpv4Pool.NotFound / "The pool ID '...' does not exist."
6. Re-replay the four check ids in trace_ids to confirm.
