Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-round-trip-and-delete apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-invalid-target-arn-rejected-and-rolled-back
Issue-Key: apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-round-trip-and-delete|_assert_link_and_nlb#11|AssertionError|assert [x["id"] for x in listed] == [out["VpcLinkId"]], (
Issue-Key: apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-invalid-target-arn-rejected-and-rolled-back|_assert_link_and_nlb#12|AssertionError|assert [x["id"] for x in listed] == [out["VpcLinkId"]], (
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigateway__cloudformation__ec2__elbv2: AWS::ApiGateway::VpcLink creates no link discoverable by GetVpcLinks and Ref returns the link's name instead of its id

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcLink

## Description
Cause 1 of 1: a CloudFormation-provisioned AWS::ApiGateway::VpcLink leaves no VPC link in the API Gateway control plane, and its Ref resolves to the configured name rather than the generated link id.
AWS: AWS::ApiGateway::VpcLink creates an API Gateway VPC link over the named NLB target ARNs. After the stack completes, Smithy apigateway GetVpcLinks (GET /vpclinks, output VpcLinks with items/jsonName "item") lists that link, each item carrying the generated `id`, the `name` and `targetArns`; Ref on the logical resource resolves to that generated VPC link **id** (the value GetVpcLink/PutIntegration's connectionId take), not to the link's Name. A VpcLink whose TargetArns name a nonexistent NLB settles FAILED and the stack rolls back.
Emulator: after the stack reports success, GetVpcLinks lists NO link with the configured name (the filtered list is empty), and Ref on the resource resolves to the link's name string. The same two symptoms appear on the round-trip/delete path and on the invalid-target-ARN rollback path.
Why: a stack output or a PutIntegration connectionId taken from Ref is a name, not an id, so the private integration can never be wired; and because the link is not discoverable through GetVpcLinks, a caller can neither verify it exists nor delete it, and cannot tell a rolled-back invalid-target stack from a successful one.
Fix: the CloudFormation AWS::ApiGateway::VpcLink resource handler — it must create the link in the apigateway VPC-link state (so GetVpcLinks/GetVpcLink see it), expose the generated id as the resource's Ref/physical id, and settle a link over a nonexistent NLB as FAILED so the stack rolls back.
Traces: apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-round-trip-and-delete, apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-invalid-target-arn-rejected-and-rolled-back

## Evidence
"AssertionError: VPC links named 'sim2real-behav-vpclink-4b20f96282d6': [], Ref 'sim2real-behav-vpclink-4b20f96282d6'" and "VPC links named 'sim2real-behav-vpclinkgood-2cb16d19de7f': [], Ref 'sim2real-behav-vpclinkgood-2cb16d19de7f'".
Expected: GetVpcLinks lists an item whose name is the configured name and whose id is a generated API Gateway VPC link id, and Ref resolves to that id. Actual: no item is listed and Ref is the name. The names shown are per-run values; the requirement is that the link is listed and Ref is the id.

## Steps to Reproduce
1. Create an NLB (elbv2) and note its ARN.
2. Deploy a CloudFormation stack with an AWS::ApiGateway::VpcLink whose Name is a known value and TargetArns is that NLB ARN, plus an Output of Ref on the resource.
3. apigateway GetVpcLinks and filter by that name; read the stack Output.
Expected: one item with that name and a generated id; the Output equals that id. Actual: no item, Output equals the name.
4. Repeat with TargetArns naming a nonexistent NLB: expect the link to settle FAILED and the stack to roll back.
