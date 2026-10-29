Issue-Source: trace-sweep-divergence
Issue-Trace: elbclassic-subnet-attach-detach-and-security-group-reassignment
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbclassic-subnet-attach-detach-and-security-group-reassignment
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: elasticloadbalancing|DetachLoadBalancerFromSubnets|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|AttachLoadBalancerToSubnets|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|ApplySecurityGroupsToLoadBalancer|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 47f32ebbccbde7752714345af879a016dd1bff94

# ELB Classic error replies carry the generic error-shape doc sentence instead of the request-specific message

> _Note: this issue was identified against the code base at commit `47f32ebbccbde7752714345af879a016dd1bff94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetachLoadBalancerFromSubnets

## Description
Three ELB Classic validation failures return the right HTTP status and the right error code but fill ErrorResponse.Error.Message with the one-sentence summary of the error SHAPE (literally the Smithy doc string for InvalidConfigurationRequestException / SubnetNotFoundException / InvalidSecurityGroupException) instead of the message AWS composes for the specific request. The message is part of the wire contract, so each of these is a divergence. 3 causes.

Cause 1 of 3: DetachLoadBalancerFromSubnets detach-all-subnets rejection (fixture elbclassic-subnet-attach-detach-and-security-group-reassignment, step task#11).
AWS: Smithy: DetachLoadBalancerFromSubnets errors AccessPointNotFoundException (LoadBalancerNotFound, 400) and InvalidConfigurationRequestException (awsQueryError code InvalidConfigurationRequest, httpError 409); the shape's doc summary is "The requested configuration change is not valid." For an attempt to detach every subnet of a VPC load balancer, the recorded response is 409 / InvalidConfigurationRequest with the request-specific message: fixed text `Requested configuration change for LoadBalancer "` + the LoadBalancerName from the request + `" is invalid because you attempted to detach all the subnets for this LoadBalancer and a LoadBalancer cannot be attached to zero subnets in VPC.` The only variable part is the load balancer name taken from the request (the recording's sim2real-probe-clb-... name is a generated value filling that slot, not part of the requirement).
Emulator: 409 / InvalidConfigurationRequest with Message = "The requested configuration change is not valid." — the shape doc summary, with no load balancer name and no statement of which rule was violated.
Why: InvalidConfigurationRequest is raised for many distinct rules on this operation; clients and operators disambiguate them by message text. A client that matches the message to decide whether to retry with one subnet left, or that logs the message for an operator, gets an unactionable sentence and cannot tell a zero-subnet rejection from any other configuration rejection.
Emulator component Fix: the DetachLoadBalancerFromSubnets handler's validation — when the detach would leave the load balancer with zero subnets, raise InvalidConfigurationRequest with the message template above, interpolating the request's LoadBalancerName.
Traces: elbclassic-subnet-attach-detach-and-security-group-reassignment#task#11.

Cause 2 of 3: AttachLoadBalancerToSubnets unknown-subnet rejection (fixture elbclassic-subnet-attach-detach-and-security-group-reassignment, step task#14).
AWS: Smithy: AttachLoadBalancerToSubnets errors include SubnetNotFoundException (awsQueryError code SubnetNotFound, httpError 400) whose doc summary is "One or more of the specified subnets do not exist.", and InvalidSubnetException (InvalidSubnet, 400). Recorded behavior only for the message: the recorded 400 response carries a request-specific Message that the corpus elides (shape-asserted, '<shaped-not-asserted>') because it embeds values from the request; the recorded message is therefore NOT the shape's generic summary. Hypothesis: AWS names the offending subnet id(s) from the request in the message, as it does for the load-balancer name in cause 1.
Emulator: 400 with Message = "One or more of the specified subnets do not exist." — again the shape doc summary verbatim, carrying none of the requested subnet ids.
Why: a client attaching several subnets cannot tell WHICH subnet id was rejected, so it must re-probe each subnet to recover; and because the emulator reuses the doc sentence for every subnet failure, SubnetNotFound and InvalidSubnet become indistinguishable by message.
Fix: the AttachLoadBalancerToSubnets handler's subnet validation and its error serializer — build the message from the rejected subnet id(s) in the request instead of emitting the error shape's documentation string. Re-record/confirm against the fixture step rather than hard-coding the elided value.
Traces: elbclassic-subnet-attach-detach-and-security-group-reassignment#task#14.

Cause 3 of 3: ApplySecurityGroupsToLoadBalancer unknown-security-group rejection (fixture elbclassic-subnet-attach-detach-and-security-group-reassignment, step task#15).
AWS: Smithy: ApplySecurityGroupsToLoadBalancer errors include InvalidSecurityGroupException (awsQueryError code InvalidSecurityGroup, httpError 400) whose doc summary is "One or more of the specified security groups do not exist." Recorded behavior only for the message: the recorded 400 response's Message is elided by the corpus ('<shaped-not-asserted>') because it carries request values, so it differs from the generic shape summary. Hypothesis: the message names the security group id(s) from the request.
Emulator: 400 with Message = "One or more of the specified security groups do not exist." — the shape doc summary, with no security group id.
Why: same loss as cause 2 for security-group reassignment: the caller cannot identify the offending group id from the error and cannot distinguish a non-existent group from a group in the wrong VPC.
Fix: the ApplySecurityGroupsToLoadBalancer handler's security-group validation and its error serializer — compose the message from the rejected group id(s) in the request; confirm against the fixture step.
Traces: elbclassic-subnet-attach-detach-and-security-group-reassignment#task#15.

Shared root cause: the error serializer appears to default Error.Message to the modelled exception shape's documentation summary. Note: the two open elasticloadbalancing issues on record (ELBv2 trust store actions answering 400 InvalidAction) concern a different API surface and do not cover these ELB Classic messages.

## Evidence
All three steps come from fixture elbclassic-subnet-attach-detach-and-security-group-reassignment; status matched in every case, only Error.Message diverged.
Cause 1 — DetachLoadBalancerFromSubnets, expected 409 / actual 409: "$.ErrorResponse.Error.Message: value_mismatch (expected 'Requested configuration change for LoadBalancer \"sim2real-probe-clb-b971b74e\" is invalid because you attempted to detach all the subnets for this LoadBalancer and a LoadBalancer cannot be attached to zero subnets in VPC.', got 'The requested configuration change is not valid.')". The actual string is exactly the Smithy doc of InvalidConfigurationRequestException. Fixed text = everything but the quoted load balancer name; variable = the LoadBalancerName from the request.
Cause 2 — AttachLoadBalancerToSubnets, expected 400 / actual 400: "$.ErrorResponse.Error.Message: value_mismatch (expected '<shaped-not-asserted>', got 'One or more of the specified subnets do not exist.')". The actual string is exactly the Smithy doc of SubnetNotFoundException; the recorded message is request-specific and was elided.
Cause 3 — ApplySecurityGroupsToLoadBalancer, expected 400 / actual 400: "$.ErrorResponse.Error.Message: value_mismatch (expected '<shaped-not-asserted>', got 'One or more of the specified security groups do not exist.')". The actual string is exactly the Smithy doc of InvalidSecurityGroupException.

## Steps to Reproduce
1. Replay fixture elbclassic-subnet-attach-detach-and-security-group-reassignment against the emulator.
2. Step task#11: on a VPC classic load balancer, call DetachLoadBalancerFromSubnets with every attached subnet. The emulator answers 409 InvalidConfigurationRequest with Message 'The requested configuration change is not valid.' instead of the message naming the load balancer and the zero-subnets rule.
3. Step task#14: call AttachLoadBalancerToSubnets with a subnet id that does not exist. The emulator answers 400 with Message 'One or more of the specified subnets do not exist.' instead of the recorded request-specific message.
4. Step task#15: call ApplySecurityGroupsToLoadBalancer with a security group id that does not exist. The emulator answers 400 with Message 'One or more of the specified security groups do not exist.' instead of the recorded request-specific message.
