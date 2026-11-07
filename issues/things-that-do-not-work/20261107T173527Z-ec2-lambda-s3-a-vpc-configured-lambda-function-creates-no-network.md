Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__s3.e2e-neptune-bridge-carve-lifecycle
Issue-Key: ec2__lambda__s3.e2e-neptune-bridge-carve-lifecycle|phase#7|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: edd8d27229a8bc99e308a4b32d393f13d6f32cb7

# A VPC-configured Lambda function creates no network interface, so DescribeNetworkInterfaces never shows its Hyperplane ENI

> _Note: this issue was identified against the code base at commit `edd8d27229a8bc99e308a4b32d393f13d6f32cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeNetworkInterfaces

## Description
Cause 1 of 1: a Lambda function created with a VpcConfig (subnet + security group) never produces the EC2 network interface that carries its VPC traffic.
AWS: creating a function with VpcConfig makes Lambda provision a Hyperplane network interface in the named subnet/security-group pair; the function reaches Active only once that attachment exists, and EC2 DescribeNetworkInterfaces reports a requester-managed interface in that subnet attributable to the function (description naming the Lambda VPC ENI and the function's security group). That ENI is the access path by which the in-VPC function reaches S3 through a gateway endpoint with no NAT and no internet route. (Recorded behavior for the interface's description text; Smithy models the interface shape but not the Lambda-side wording.)
Emulator: the solve phase of the lifecycle finds no such interface at all — DescribeNetworkInterfaces returns `[]` for 120s — so the whole access path under test (invoke the in-VPC function from outside the VPC and have it read the seeded S3 objects through the gateway endpoint) cannot be established. The open issues 20261105T223150Z-ec2-lambda-a-vpc-configured-lambda-function-creates.md and 20261029T065746Z-ec2-lambda-a-vpc-function-s-vpcconfig-never.md describe the same family under the `ec2__lambda` heading but do not carry this check's replay key.
Why: every VPC-attached Lambda topology is unverifiable: a client cannot see (or clean up) the ENI the function holds, cannot correlate the function with its subnet/security group, and cannot reason about in-VPC egress through endpoints.
Fix: the Lambda CreateFunction/UpdateFunctionConfiguration VPC path must create a requester-managed network interface in each configured subnet and register it in EC2's network-interface state (and release it when the function is deleted).
Traces: ec2__lambda__s3.e2e-neptune-bridge-carve-lifecycle (phase 4/7 `solve`).

## Evidence
Expected: a network interface attributable to the VPC-configured bridge function appears in DescribeNetworkInterfaces within the deadline.
Actual: `AssertionError: [phase 4/7 solve] the Hyperplane ENI for sim2real-xsvc-npt-e2e-<id>-bridge not met within 120.0s. Last result: []` (the function name is a per-run value; the requirement is that the interface exists and is attributable to the function's subnet/security group).

## Steps to Reproduce
1. Replay the behavioral suite and look under `### ec2__lambda__s3` for e2e-neptune-bridge-carve-lifecycle.
2. Create a VPC with a subnet, a security group and an S3 gateway endpoint (no NAT/IGW), then create a Lambda function with VpcConfig naming that subnet and security group.
3. Wait for the function configuration to report Active, then call ec2 DescribeNetworkInterfaces scoped to that subnet/security group — no interface is returned.
