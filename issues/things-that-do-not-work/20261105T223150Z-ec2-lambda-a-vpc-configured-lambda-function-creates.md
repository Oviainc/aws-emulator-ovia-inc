Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda.xsvc-ec2-3-lambda-vpc-role-without-create-network-interface-rejected
Issue-Key: ec2__lambda.xsvc-ec2-3-lambda-vpc-role-without-create-network-interface-rejected|_poll#31|TimeoutError|raise TimeoutError(f"{what}: the run budget ended first; not graded")
Issue-Commit: 2730cd672dd608c80462ed6cac2962eb48b48789

# ec2__lambda: a VPC-configured Lambda function creates no network interface in the VPC, so no ENI carrying its security group ever appears to DescribeNetworkInterfaces

> _Note: this issue was identified against the code base at commit `2730cd672dd608c80462ed6cac2962eb48b48789`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction (VpcConfig) / DescribeNetworkInterfaces

## Description
Cause 1 of 1: a function created with a VpcConfig naming subnets and a security group never materialises a network interface in that VPC, so the EC2 view of the function's VPC attachment is permanently empty and the scenario that grades the role-permission requirement for ENI creation cannot even establish its control.

AWS: recorded behavior only for the attachment side effect (no single modelled operation expresses it). Smithy (service-com-amazonaws-lambda-awsgirapiservice) does model the coupling: Invoke carries the VPC/ENI failure modes as modelled errors — EC2AccessDeniedException (502), ENILimitReachedException (502), ENINotReadyException (502), InvalidSecurityGroupIDException (502), InvalidSubnetIDException (502), SubnetIPAddressLimitReachedException (502) — which only exist because a real VPC attachment is created and can fail. AWS's documented behavior: when a function is configured with VpcConfig, Lambda creates a requester-managed elastic network interface in the named subnet(s) carrying the named security group(s); that interface is visible through ec2:DescribeNetworkInterfaces filtered on the security group, and the function's VPC attachment converges to usable. A function whose EXECUTION ROLE lacks ec2:CreateNetworkInterface cannot complete that attachment and is refused rather than served, which is exactly the contrast this check grades against a sibling role that holds the permission.

Emulator: no interface is ever created. Polling ec2:DescribeNetworkInterfaces for at least one interface carrying the function's security group runs to the end of the run budget and the check aborts:
  TimeoutError: an ENI carrying <sg-id> appearing: the run budget ended first; not graded
Because the positive control (a role that DOES hold ec2:CreateNetworkInterface producing an ENI) never appears, the negative leg — the ungranted role being refused — is never reached, and no status, error code or message is produced for comparison.

Why: a client relying on AWS's behavior sees a VPC-configured function that reports as configured but has no presence in the VPC. Nothing enforces or exposes the role-permission requirement for VPC attachment, so a function whose execution role is missing ec2:CreateNetworkInterface looks healthy here and fails on real AWS; and nothing in the EC2 view can be used to check subnet/security-group wiring, ENI cleanup on DeleteFunction, or the ENI-dependency refusals (deleting a subnet or security group still in use by a function's interface).

Fix: the Lambda provisioner's VpcConfig handling — attaching a function to a VPC must create a requester-managed network interface per configured subnet in the EC2 service's state, carrying the configured security groups, and must refuse the attachment when the execution role does not allow ec2:CreateNetworkInterface rather than reporting the function as attached.

Traces: ec2__lambda.xsvc-ec2-3-lambda-vpc-role-without-create-network-interface-rejected. The open issue 20261029T065746Z-ec2-lambda-a-vpc-function-s-vpcconfig-never.md covers the sibling check xsvc-ec2-3-lambda-vpc-eni-dependency-violation-rejected but does not carry this check's replay key.

## Evidence
Expected (AWS): after creating a function with VpcConfig={'SubnetIds':[<subnet>],'SecurityGroupIds':[<sg>]} and an execution role allowing ec2:CreateNetworkInterface/DescribeNetworkInterfaces/DeleteNetworkInterface, ec2:DescribeNetworkInterfaces filtered on <sg> returns at least one requester-managed interface in <subnet>; the sibling function whose role omits ec2:CreateNetworkInterface is refused its VPC attachment instead of being served.

Actual (emulator): the poll for the first interface never succeeds and the check aborts without grading:
  File "check_ec2__lambda_xsvc_ec2_3_lambda_vpc_role_without_create_network_interface_rejected.py", line 99, in run
    enis = _poll(ctx, lambda: _enis_with_group(ec2, sg), lambda e: len(e) >= 1, 300, 1, ...)
  TimeoutError: an ENI carrying sg-0000000000000003f appearing: the run budget ended first; not graded
(The sg id shown is this run's generated value, not a requirement.)

## Steps to Reproduce
1. Create a VPC, a subnet and a security group.
2. Create an execution role that trusts lambda.amazonaws.com and allows ec2:CreateNetworkInterface, ec2:DescribeNetworkInterfaces and ec2:DeleteNetworkInterface.
3. CreateFunction with VpcConfig={'SubnetIds':[<subnet-id>],'SecurityGroupIds':[<sg-id>]} using that role; wait for the function to become Active.
4. Call ec2:DescribeNetworkInterfaces with Filters=[{'Name':'group-id','Values':[<sg-id>]}]. Expect at least one interface; observe the list stays empty indefinitely.
5. Repeat step 3 with a second role that omits ec2:CreateNetworkInterface and observe that the VPC attachment is not refused.
