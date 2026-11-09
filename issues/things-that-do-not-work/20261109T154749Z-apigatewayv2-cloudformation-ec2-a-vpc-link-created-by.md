Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation__ec2.deploy-apigatewayv2-vpclink-round-trip-then-delete apigatewayv2__cloudformation__ec2.deploy-apigatewayv2-vpclink-mismatched-vpc-security-group-rejected
Issue-Key: apigatewayv2__cloudformation__ec2.deploy-apigatewayv2-vpclink-round-trip-then-delete|run#13|AssertionError|assert len(matches) == 1, f"GetVpcLinks found {matches!r} for Name {link_name!r}"
Issue-Key: apigatewayv2__cloudformation__ec2.deploy-apigatewayv2-vpclink-mismatched-vpc-security-group-rejected|run#13|AssertionError|assert listed == [good_out["VpcLinkId"]], f"GetVpcLinks found {listed!r} for the control"
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigatewayv2__cloudformation__ec2: a VPC link created by a CloudFormation AWS::ApiGatewayV2::VpcLink resource is invisible to GetVpcLinks, and a mismatched-VPC security group is not refused

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVpcLinks

## Description
Cause 1 of 1: a stack that declares AWS::ApiGatewayV2::VpcLink reaches a terminal state without the VPC link appearing in the apigatewayv2 control plane, so neither the round-trip nor the refusal contract can be observed.

AWS: Smithy (service-com-amazonaws-apigatewayv2-apigatewayv2): GetVpcLinks — GET /v2/vpclinks, output Items : __listOfVpcLink, each VpcLink carrying VpcLinkId, Name, SubnetIds, SecurityGroupIds, VpcLinkStatus and VpcLinkVersion; CreateVpcLink's modeled errors are BadRequestException (400), ConflictException (409), NotFoundException (404), TooManyRequestsException (429). A CloudFormation-created VPC link is an ordinary API Gateway resource: after CREATE_COMPLETE, GetVpcLinks lists it by the stack's Name and Ref is its VpcLinkId; AWS also refuses a link whose SecurityGroupIds belong to a different VPC than its SubnetIds with 400 BadRequestException, so that stack ends CREATE_FAILED/ROLLBACK_COMPLETE while the same template with matching ids deploys and is listed.
Emulator: after the stack settles, GetVpcLinks returns an empty Items list — the resource the stack claims to have created is unknown to the control plane ("GetVpcLinks found [] for Name '<name>'", and "GetVpcLinks found [] for the control" on the positive control of the mismatched-VPC check, so the refusal leg cannot be graded either).
Why: a client cannot discover, update or delete a VPC link its own stack created, and a private-integration estate deployed by CloudFormation is unusable; the mismatched-VPC refusal — a deploy-time guardrail — is also unverifiable.
Fix: the AWS::ApiGatewayV2::VpcLink resource handler in the cloudformation crate must create the link through the apigatewayv2 state the GetVpcLink/GetVpcLinks handlers read (shared state, not a private copy), and CreateVpcLink's subnet/security-group VPC-consistency validation must refuse the mismatched case with BadRequestException.
Related but not covering: 20261104T121154Z-apigatewayv2-3-net-new-root-causes-a-cfn.md reports a CFN-created VpcLink unknown to GetVpcLink under service apigatewayv2, and 20261104T121154Z-apigatewayv2-getvpclinks-stub.md concerns the GetVpcLinks stub; neither owns these replay keys and GetVpcLinks here answers 200 with an empty list rather than NotImplemented.
Traces: apigatewayv2__cloudformation__ec2.deploy-apigatewayv2-vpclink-round-trip-then-delete, apigatewayv2__cloudformation__ec2.deploy-apigatewayv2-vpclink-mismatched-vpc-security-group-rejected.

## Evidence
Expected: CreateStack with AWS::ApiGatewayV2::VpcLink (Name, SubnetIds, SecurityGroupIds of the same VPC) -> CREATE_COMPLETE, then GetVpcLinks Items contains an entry whose Name is the stack's name and whose VpcLinkId equals Ref; a template whose SecurityGroupIds are from another VPC fails the stack with API Gateway's 400 BadRequestException. Actual: AssertionError "GetVpcLinks found [] for Name 'sim2real-apigwv2vl-link-<id>'" and AssertionError "GetVpcLinks found [] for the control" — the control plane lists no VPC link at all.

## Steps to Reproduce
1. Create a VPC, subnet and security group via ec2.
2. CreateStack with a single AWS::ApiGatewayV2::VpcLink naming them; wait for the terminal status.
3. Call apigatewayv2 GetVpcLinks — Items is empty although the stack reports the resource created.
4. Repeat with SecurityGroupIds from a second VPC — the stack is not failed with BadRequestException.
5. Replay apigatewayv2__cloudformation__ec2.deploy-apigatewayv2-vpclink-round-trip-then-delete and apigatewayv2__cloudformation__ec2.deploy-apigatewayv2-vpclink-mismatched-vpc-security-group-rejected.
