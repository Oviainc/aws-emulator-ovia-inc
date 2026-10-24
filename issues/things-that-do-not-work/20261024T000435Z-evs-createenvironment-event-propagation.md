Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# The ten VLAN subnets CreateEnvironment provisions are never created in the account's EC2/VPC store

_Filed: 2026-10-24T00:04:35Z_
_Operation: CreateEnvironment | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Amazon EVS 'provisions the VLAN subnets' inside the VPC the request names, and 'Environment deletion also deletes the associated Amazon EVS VLAN subnets' — they are real VPC subnets in the caller's account, each with an availability zone. Expected: after CreateEnvironment the ten subnet ids reported on the environment's VLANs exist in EC2 (ec2 DescribeSubnets finds them in `vpcId`, with the CIDR the request gave), each `Vlan.availabilityZone` reports the AZ of that subnet, and DeleteEnvironment removes them. Actual: `state.mint_subnet_id()` fabricates `subnet-…` ids that are stored only inside the EVS record; nothing is written to the shared engine's EC2 store (the EVS crate reads EC2 for vpc/subnet/EIP existence but never writes it), so the ids name resources no other service holds, `Vlan.availabilityZone` is hard-coded absent because there is no subnet to read it from, and deleting the environment removes nothing from EC2.

## Evidence

Spec: CreateEnvironment doc ('Amazon EVS provisions only the VLAN subnets' in SELF_DEPLOYED mode, and provisions them plus VCF otherwise), DeleteEnvironment doc ('Environment deletion also deletes the associated Amazon EVS VLAN subnets'), `Vlan.availabilityZone` / `Vlan.subnetId` members. Handler: src/ops/create_environment.rs lines 367-385 (`subnet_id: state.mint_subnet_id()`); src/state.rs `mint_subnet_id` (line 1256) and `StoredVlan::to_wire` lines 774-789 ('availabilityZone is left absent ... this emulator holds no EC2 state to read it from' — while src/state/ec2_resources.rs shows the EC2 store IS reachable from this crate); src/ops/delete_environment.rs only flips VLAN states. Tests: no test asserts the minted subnet ids exist in EC2 (tests/all/* register only the VPC and the service access subnet via `register_network`).

## Steps to Reproduce

1. Register a VPC and a service access subnet in the shared EC2 store, then CreateEnvironment against them.
2. Read the environment's VLANs with ListEnvironmentVlans and note the ten `subnetId` values (and that `availabilityZone` is absent).
3. Call ec2 DescribeSubnets on the same engine for those ids — none of them exists.
4. DeleteEnvironment and observe that no EC2 subnet is removed, since none was ever created.
