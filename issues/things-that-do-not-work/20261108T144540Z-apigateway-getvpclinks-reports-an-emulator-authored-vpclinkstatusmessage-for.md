Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-vpc-link-errors
Issue-Step: task#4
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-vpc-link-errors
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|GetVpcLinks|2xx->2xx|$.items[name="sim2real-wire-vpclink-e4e19d6b"].vpcLinkStatusMessage
Issue-Commit: e1db6a8e1942cc9ea3bd6edbf0449682b4a8a46d

# apigateway GetVpcLinks reports an emulator-authored vpcLinkStatusMessage for an AVAILABLE VPC link

> _Note: this issue was identified against the code base at commit `e1db6a8e1942cc9ea3bd6edbf0449682b4a8a46d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVpcLinks

## Description
Cause 1 of 1: the VPC-link status message served on the list/describe path is emulator-authored prose instead of AWS's fixed text.

AWS: GetVpcLinks returns a collection of VpcLink representations. Smithy (com.amazonaws.apigatewayv2#VpcLink): member VpcLinkStatus -> enum VpcLinkStatus with values PENDING | AVAILABLE | DELETING | FAILED | INACTIVE, and member VpcLinkStatusMessage -> StringWithLengthBetween0And1024, jsonName 'vpcLinkStatusMessage', documented as 'A message summarizing the cause of the status of the VPC link.' The model does not enumerate the message strings, so for the specific text this is recorded behavior only: the recording shows that when a VPC link has reached the AVAILABLE status, AWS populates vpcLinkStatusMessage with the fixed sentence 'VPC link is ready to route traffic'. That sentence is entirely fixed text for the AVAILABLE status class -- it interpolates no name, id, subnet, security group or timestamp from the request or from state -- so it is reproducible verbatim and is a requirement, not a per-run value. (The analogous REST-API shape com.amazonaws.apigateway#VpcLink carries the same pair as status/statusMessage.)

Emulator: the same member comes back as 'The VPC link is available for use by an integration.' -- a different, emulator-invented sentence for the same AVAILABLE status. Status is 200 and every other member of the item matches, so the divergence is purely the status-message text the emulator synthesises when it projects a VPC link into a response.

Why: vpcLinkStatusMessage is the human-readable half of the VPC-link status contract and is what tooling surfaces, logs and asserts on while polling a VPC link to readiness. A client (or a test/readiness gate) that matches AWS's 'VPC link is ready to route traffic' never matches against the emulator, so a link that is in fact AVAILABLE is reported as not-yet-ready, and any diagnostic that echoes the message to a user shows wording AWS never emits. Because the text is status-derived and not request-derived, there is no environment-dependent excuse for the difference.

Fix: the VPC-link response projection/serializer for the apigateway VPC-link resource -- the code that derives statusMessage/vpcLinkStatusMessage from the stored VpcLinkStatus. The per-status message strings must be AWS's, applied consistently to GetVpcLinks, GetVpcLink and CreateVpcLink, not only to the list path.

Note: the open issue 20261029T200808Z-apigateway-api-gateway-tag-vpc-link-stage-and-import.md covers CreateVpcLink's malformed-ARN refusal and DeleteVpcLink's not-found message; it does not cover the status-message text of a successful VPC-link representation, and the replay reports this cluster as unowned.

Traces: apigatewayv2-vpc-link-errors#task#4.

## Evidence
Replay cluster (verdict mismatch, service=apigateway, operation=GetVpcLinks, status 200 -> 200):
  $.items[name="sim2real-wire-vpclink-<generated>"].vpcLinkStatusMessage: value_mismatch
    expected: 'VPC link is ready to route traffic'
    actual:   'The VPC link is available for use by an integration.'

Fixed vs variable parts of the message: for the AVAILABLE status the whole sentence 'VPC link is ready to route traffic' is fixed text -- there are no variable parts taken from the request or from state. The VPC link name in the JSONPath selector above is a per-run generated name and is NOT part of the requirement; only the message value is.

All other asserted members of the item (name, id, status, subnets/security groups) matched, and the HTTP status matched at 200, so the emulator's state and status enum are correct and only the derived status message diverges.

## Steps to Reproduce
1. Clone the repo and replay the corpus: replay_wire_corpus().
2. Locate fixture apigatewayv2-vpc-link-errors, step task#4 (the GetVpcLinks call after the VPC link has reached AVAILABLE).
3. Equivalently by hand: create a VPC link, wait until its status is AVAILABLE, then GET the VPC-link collection and read items[].vpcLinkStatusMessage (status/statusMessage on the REST-API shape).
4. Observe the emulator answers 200 with vpcLinkStatusMessage 'The VPC link is available for use by an integration.' where AWS answers 200 with vpcLinkStatusMessage 'VPC link is ready to route traffic'.
