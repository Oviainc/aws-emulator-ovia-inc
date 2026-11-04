Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__events__sqs.emr-step-status-change-delivers-failed-and-withholds-it-from-the-completed-rule
Issue-Key: emr__events__sqs|CreateVpcEndpoint|NotImplemented
Issue-Commit: 685d7dc1b36320cc963844050fe238af8df0451a

# EC2 CreateVpcEndpoint refuses any request carrying RouteTableIds with 501 NotImplemented, blocking the EMR step-status-change event scenario before any EMR call is made

> _Note: this issue was identified against the code base at commit `685d7dc1b36320cc963844050fe238af8df0451a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
Cause 1 of 1: CreateVpcEndpoint rejects the whole request as soon as route-table associations are present, with an emulator-authored NotImplemented refusal.

AWS: Smithy (com.amazonaws.ec2#AmazonEC2): CreateVpcEndpoint input includes RouteTableIds (VpcEndpointRouteTableIdList, xmlName RouteTableId) alongside VpcId (required) and ServiceName; it is a normal, fully supported member for a Gateway endpoint. AWS answers HTTP 200 with CreateVpcEndpointResult.VpcEndpoint, whose RouteTableIds echo the associated route tables, and installs the prefix-list route in each named route table. CreateVpcEndpoint declares no modeled errors for this input; a refusal would be a 400 with a Client.* error code, never a 501 NotImplemented.

Emulator: HTTP 501 with error code NotImplemented and the emulator-authored message 'operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator'. Neither the status, the code, nor the message text is anything AWS produces for this request.

Why: any scenario whose setup places a Gateway endpoint on a VPC's route tables dies at that call. Here the EMR scenario never gets to run a cluster step, so 'EMR Step Status Change' event delivery (FAILED routed to the matching rule and withheld from the completed rule) is never exercised; a client doing the same wiring sees a 501 for a request AWS accepts and cannot fall back, because NotImplemented is not a retryable or correctable condition.

Fix: the ec2 crate's CreateVpcEndpoint handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_endpoint.rs) — associate the named route tables and serialize RouteTableIds on the returned VpcEndpoint instead of refusing the request.

Note: the open issue 20261104T044826Z-ec2-lambda-s3-sqs-createvpcendpoint-refuses-route-table-associations-and-masks-the.md describes this refusal for checks under the ec2__lambda__s3__sqs heading; it does not carry this check's replay key, so this failure is not covered by it and the check below must be re-replayed to confirm the fix.

Traces: emr__events__sqs.emr-step-status-change-delivers-failed-and-withholds-it-from-the-completed-rule.

## Evidence
Behavioral suite replay against the emulator built from this checkout.

Check emr__events__sqs.emr-step-status-change-delivers-failed-and-withholds-it-from-the-completed-rule [error]:
  expected: CreateVpcEndpoint with VpcId, ServiceName and RouteTableIds -> HTTP 200 with VpcEndpoint.RouteTableIds listing the named route tables.
  actual:   botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateVpcEndpoint operation: operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator.

Fixed text of the divergence: status 501 and error code 'NotImplemented' where AWS answers 200; the message is entirely emulator-authored (AWS emits no such message for this operation).

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from this checkout.
2. Replay check emr__events__sqs.emr-step-status-change-delivers-failed-and-withholds-it-from-the-completed-rule.
3. Manual repro: CreateVpc, CreateSubnet, then CreateVpcEndpoint(VpcId=<vpc>, VpcEndpointType='Gateway', ServiceName='com.amazonaws.<region>.s3', RouteTableIds=[<route-table-id>]). Expect 200 with VpcEndpoint.RouteTableIds=[<route-table-id>]; observe 501 NotImplemented.
