Issue-Source: trace-sweep-divergence
Issue-Trace: elbclassic-subnet-attach-detach-and-security-group-reassignment
Issue-Step: task#14
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbclassic-subnet-attach-detach-and-security-group-reassignment
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: elasticloadbalancing|AttachLoadBalancerToSubnets|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|ApplySecurityGroupsToLoadBalancer|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: a57dea262f41459244daf18c6d5b4cdf00e6a648

# ELB Classic refusals carry EC2's wording instead of the modeled ELB message

> _Note: this issue was identified against the code base at commit `a57dea262f41459244daf18c6d5b4cdf00e6a648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachLoadBalancerToSubnets

## Description
Two Classic Load Balancer operations that must refuse a request naming a network resource the account does not hold answer the right status and error code, but put EC2's own refusal sentence in Error.Message instead of the sentence ELB words for its own error class. Both causes share one root: the handlers forward NetworkRefusal::reported_message() (the sentence the EC2 lookup answered) straight into the ELB error shape's Message member.

Cause 1 of 2 — AttachLoadBalancerToSubnets, subnet that does not exist.
AWS: Smithy: AttachLoadBalancerToSubnets declares SubnetNotFoundException, awsQueryError code "SubnetNotFound", httpError 400, with the documented class "One or more of the specified subnets do not exist." The recorded AWS response for this step is HTTP 400 with that ELB-worded message; the corpus asserts the message's shape (expected value '<shaped-not-asserted>'), i.e. the refusal sentence ELB itself emits for the class, whose fixed text is ELB's own and whose only variable parts are the subnet id(s) the request named. An id from one run fills that template and is not part of it.
Emulator: answers HTTP 400 SubnetNotFound (status and code correct) with Error.Message "The subnet ID 'subnet-0123456789abcdef0' does not exist" — EC2's InvalidSubnetID.NotFound wording, passed through verbatim from the EC2 subnet lookup. Replay reports $.ErrorResponse.Error.Message: value_mismatch.

Cause 2 of 2 — ApplySecurityGroupsToLoadBalancer, security group that does not exist.
AWS: Smithy: ApplySecurityGroupsToLoadBalancer declares InvalidSecurityGroupException, awsQueryError code "InvalidSecurityGroup", httpError 400, documented as "One or more of the specified security groups do not exist." The recorded response is HTTP 400 with ELB's own sentence for that class (again shape-asserted, not value-asserted); its variable part is the security group id(s) from the request.
Emulator: answers HTTP 400 InvalidSecurityGroup (status and code correct) with Error.Message "The security group 'sg-0123456789abcdef0' does not exist" — EC2's InvalidGroup.NotFound wording forwarded from the EC2 security-group lookup. Replay reports $.ErrorResponse.Error.Message: value_mismatch.

Why: an error message is part of the contract. A client, CLI or IaC tool that matches on the ELB refusal text (log scraping, retry/branch logic, user-facing diagnostics, test assertions pinned to ELB's wording) does not recognise the EC2 sentence, so it reports an unexpected failure or takes the wrong branch even though the status and code are right. It also leaks the cross-service implementation detail that ELB asked EC2, which AWS's ELB surface never exposes.

Fix: the error-construction sites in the two operation handlers (not the EC2 lookup): each must word its own modeled refusal for its error class from the request's ids rather than forwarding the EC2 refusal sentence. The shared wording belongs beside this service's other refusal-message helpers in its state module, so both operations state the class identically.

Traces: elbclassic-subnet-attach-detach-and-security-group-reassignment#task#14 (AttachLoadBalancerToSubnets), elbclassic-subnet-attach-detach-and-security-group-reassignment#task#15 (ApplySecurityGroupsToLoadBalancer).

## Evidence
Cause 1 — fixture elbclassic-subnet-attach-detach-and-security-group-reassignment, step task#14, operation AttachLoadBalancerToSubnets. Expected status 400, actual status 400. Replay detail: "$.ErrorResponse.Error.Message: value_mismatch (expected '<shaped-not-asserted>', got \"The subnet ID 'subnet-0123456789abcdef0' does not exist\")". Fixed text of the emulator's sentence: "The subnet ID '...' does not exist" (EC2's wording); variable part: the subnet id from the request. Modeled requirement — Smithy: AttachLoadBalancerToSubnets error SubnetNotFoundException, awsQueryError code SubnetNotFound / httpResponseCode 400, doc "One or more of the specified subnets do not exist."
Emulator source: services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/ops/attach_load_balancer_to_subnets.rs lines 202-219 — SubnetNotFoundException { message: Some(refusal.reported_message()) }.

Cause 2 — same fixture, step task#15, operation ApplySecurityGroupsToLoadBalancer. Expected status 400, actual status 400. Replay detail: "$.ErrorResponse.Error.Message: value_mismatch (expected '<shaped-not-asserted>', got \"The security group 'sg-0123456789abcdef0' does not exist\")". Fixed text: "The security group '...' does not exist" (EC2's wording); variable part: the group id from the request. Modeled requirement — Smithy: ApplySecurityGroupsToLoadBalancer error InvalidSecurityGroupException, awsQueryError code InvalidSecurityGroup / httpResponseCode 400, doc "One or more of the specified security groups do not exist."
Emulator source: services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/ops/apply_security_groups_to_load_balancer.rs lines 199-221 — InvalidSecurityGroupException { message: Some(refusal.reported_message()) }; the forwarded sentence comes from crate::state::NetworkRefusal, re-exported from the ELBv2 crate's EC2 network lookup (src/state.rs line 26).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Fixture elbclassic-subnet-attach-detach-and-security-group-reassignment drives the ELB Classic subnet attach/detach and security-group reassignment flow against an existing load balancer.
3. Step task#14: call AttachLoadBalancerToSubnets on that load balancer naming a subnet id the account holds no subnet for. The emulator answers 400 SubnetNotFound but with Error.Message "The subnet ID '<subnet-id>' does not exist" instead of ELB's own sentence for the SubnetNotFound class.
4. Step task#15: call ApplySecurityGroupsToLoadBalancer on that load balancer naming a security group id the account does not hold. The emulator answers 400 InvalidSecurityGroup but with Error.Message "The security group '<sg-id>' does not exist" instead of ELB's own sentence for the InvalidSecurityGroup class.
5. Compare each ErrorResponse/Error/Message against the recorded AWS message for the same request.
