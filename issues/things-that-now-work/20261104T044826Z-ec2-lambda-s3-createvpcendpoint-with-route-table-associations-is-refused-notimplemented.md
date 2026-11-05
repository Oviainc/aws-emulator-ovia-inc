Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__s3.s3-gateway-endpoint-config-round-trips ec2__lambda__s3.s3-gateway-endpoint-installs-prefix-list-route ec2__lambda__s3.second-s3-gateway-endpoint-on-same-route-table-rejected ec2__lambda__s3.delete-vpc-rejected-while-vpc-endpoint-exists ec2__lambda__s3.e2e-neptune-bridge-carve-lifecycle ec2__lambda__s3.gateway-endpoint-route-from-modify-converges-asynchronously ec2__lambda__s3.new-vpc-auto-creates-default-sg-and-main-route-table ec2__lambda__s3.sg-egress-to-unknown-prefix-list-rejected ec2__lambda__s3.update-function-configuration-add-vpc-config-rejected-without-eni-permissions
Issue-Key: ec2__lambda__s3|CreateVpcEndpoint|NotImplemented
Issue-Key: ec2__lambda__s3.gateway-endpoint-route-from-modify-converges-asynchronously|run#10|AssertionError|assert endpoint["State"] == "available", (
Issue-Key: ec2__lambda__s3.new-vpc-auto-creates-default-sg-and-main-route-table|run#9|AssertionError|assert "Ipv6CidrBlockAssociationSet" not in vpc, (
Issue-Key: ec2__lambda__s3.sg-egress-to-unknown-prefix-list-rejected|run#8|AssertionError|raise AssertionError(
Issue-Key: ec2__lambda__s3.update-function-configuration-add-vpc-config-rejected-without-eni-permissions|_err#15|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it: {resp!r}")
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# CreateVpcEndpoint with route table associations is refused NotImplemented, plus four EC2/Lambda validation divergences

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
Five net-new root causes.

Cause 1 of 5: CreateVpcEndpoint refuses any Gateway endpoint that names route tables.
AWS: recorded behavior only — the recordings call ec2:CreateVpcEndpoint with VpcEndpointType=Gateway, ServiceName com.amazonaws.<region>.s3 and RouteTableIds, and get a 200 whose vpcEndpoint carries the endpoint id, State, and the RouteTableIds it associated.
Emulator: the call is refused with error code NotImplemented and message "operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator". That whole message is emulator-authored text with no AWS counterpart; AWS has no NotImplemented error for this operation, so the status and code are wrong as well as the message.
Why: every S3/DynamoDB gateway-endpoint topology is unbuildable — a client cannot create the endpoint at all, so route installation, prefix-list routes, duplicate rejection, subnet/VPC delete guards and any private-path scenario are all unreachable.
Fix: the EC2 CreateVpcEndpoint handler's route-table-association branch and the VPC endpoint / route table state it must write.
Traces: ec2__lambda__s3.s3-gateway-endpoint-config-round-trips, .s3-gateway-endpoint-installs-prefix-list-route, .second-s3-gateway-endpoint-on-same-route-table-rejected, .delete-vpc-rejected-while-vpc-endpoint-exists, .e2e-neptune-bridge-carve-lifecycle.

Cause 2 of 5: a Gateway endpoint created with no association stays State 'pending' forever.
AWS: recorded behavior only — the recording creates a Gateway endpoint with no RouteTableIds and expects DescribeVpcEndpoints to report State 'available' (availability is independent of reachability), then associates a route table by ModifyVpcEndpoint.
Emulator: the endpoint keeps State 'pending' — "AssertionError: a Gateway endpoint with no association still reports State 'pending', expected 'available'".
Why: a client that waits for State available before associating route tables waits forever; the documented value is 'available' as soon as the endpoint exists.
Fix: the VPC endpoint state machine in the EC2 CreateVpcEndpoint handler / endpoint state.
Traces: ec2__lambda__s3.gateway-endpoint-route-from-modify-converges-asynchronously.

Cause 3 of 5: DescribeVpcs reports Ipv6CidrBlockAssociationSet for an IPv4-only VPC.
AWS: recorded behavior only — the recording creates a VPC with only an IPv4 CidrBlock and asserts the Vpc shape carries no Ipv6CidrBlockAssociationSet member at all.
Emulator: the member is present on an IPv4-only VPC.
Why: a client that tests for the presence of Ipv6CidrBlockAssociationSet to decide whether a VPC is dual-stack concludes every VPC is IPv6-enabled.
Fix: the EC2 CreateVpc/DescribeVpcs serializer for the Vpc shape (omit the member when there is no IPv6 association).
Traces: ec2__lambda__s3.new-vpc-auto-creates-default-sg-and-main-route-table.

Cause 4 of 5: AuthorizeSecurityGroupEgress to an unknown prefix list is accepted.
AWS: recorded behavior only — the recording expects the call to be refused, and grades the returned error code and message; it reports "authorizing egress to an unknown prefix list unexpectedly succeeded; real AWS rejects it, so there is no error to grade". AWS refuses a PrefixListId that does not exist with the InvalidPrefixListId.NotFound error class, naming the prefix list id from the request.
Emulator: the call returns success and the rule is added.
Why: a client gets a security group with an egress rule pointing at a prefix list that does not exist, and no error telling it the id was wrong.
Fix: the EC2 AuthorizeSecurityGroupEgress handler's validation of IpPermissions[].PrefixListIds against prefix-list state.
Traces: ec2__lambda__s3.sg-egress-to-unknown-prefix-list-rejected.

Cause 5 of 5: UpdateFunctionConfiguration attaches a VpcConfig although the execution role cannot create ENIs.
AWS: recorded behavior only — the recording expects the call to be refused and grades the error; it reports "the call succeeded; real AWS rejects it" and shows a 200 whose body already carries the VpcConfig. AWS refuses attaching a VpcConfig when the function's execution role does not grant the EC2 network-interface actions, with an InvalidParameterValueException naming the missing permission on the role from the request.
Emulator: HTTP 200 with VpcConfig applied (SubnetIds/SecurityGroupIds echoed back) and LastUpdateStatus InProgress.
Why: a client gets a function that claims to be VPC-attached with a role that can never create the ENIs, and discovers it only as an unexplained invoke failure instead of at configuration time.
Fix: the Lambda UpdateFunctionConfiguration handler's VpcConfig validation (execution-role permission check) before the configuration is written.
Traces: ec2__lambda__s3.update-function-configuration-add-vpc-config-rejected-without-eni-permissions.

## Evidence
Expected (AWS): CreateVpcEndpoint with RouteTableIds -> 200 with the endpoint and its associations; a Gateway endpoint with no association -> State 'available'; an IPv4-only VPC -> no Ipv6CidrBlockAssociationSet; egress to an unknown prefix list -> InvalidPrefixListId.NotFound; UpdateFunctionConfiguration adding VpcConfig with an ENI-less role -> InvalidParameterValueException.
Actual (emulator): 'ClientError: An error occurred (NotImplemented) when calling the CreateVpcEndpoint operation: operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator'; "State 'pending'"; Ipv6CidrBlockAssociationSet present; egress authorization returns success; UpdateFunctionConfiguration returns HTTP 200 with VpcConfig set.

## Steps to Reproduce
1. CreateVpc, CreateSubnet, CreateRouteTable; call CreateVpcEndpoint(VpcEndpointType='Gateway', ServiceName='com.amazonaws.<region>.s3', RouteTableIds=[rtb-...]). Observe NotImplemented.
2. Call CreateVpcEndpoint for the same Gateway service with no RouteTableIds and DescribeVpcEndpoints. Observe State 'pending'.
3. CreateVpc with only an IPv4 CidrBlock and DescribeVpcs. Observe Ipv6CidrBlockAssociationSet is present.
4. AuthorizeSecurityGroupEgress with IpPermissions[].PrefixListIds naming a non-existent pl-... Observe success.
5. CreateFunction with a role lacking ec2:CreateNetworkInterface, then UpdateFunctionConfiguration with VpcConfig. Observe HTTP 200.
