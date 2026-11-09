Issue-Source: fixer-residual
Issue-Trace: elbclassic-subnet-attach-detach-and-security-group-reassignment
Issue-Step: task#15
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbclassic-subnet-attach-detach-and-security-group-reassignment
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: elasticloadbalancing|ApplySecurityGroupsToLoadBalancer|4xx->4xx|$.ErrorResponse.Error.Message

# ELB Classic ApplySecurityGroupsToLoadBalancer: the relayed EC2 sentence for an unresolvable security group is still not AWS's

## Operation
ApplySecurityGroupsToLoadBalancer

## Description
This is the residual of `20261104T095914Z-elasticloadbalancing-elb-classic-refusals-carry-ec2-s-wording-instead`
(now in `issues/things-that-now-work/`), whose first cause — `AttachLoadBalancerToSubnets` — is
fixed and passes replay. The rule that fixed it is now stated once for both operations
(`crate::state::relayed_network_refusal_message` in
`services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/state.rs`):
where a Classic Load Balancer operation refuses a request because EC2 would not resolve an
EC2-owned resource it named, `Error.Message` is the EC2 fault relayed WHOLE — the sentence EC2
worded about the ids the request named, then `(Service: AmazonEC2; Status Code: <status>; Error
Code: <code>; Request ID: <that call's id>; Proxy: null)`. For the subnet step the corpus now
accepts exactly that.

What is left: the INNER sentence for a security group. `ApplySecurityGroupsToLoadBalancer`
answers HTTP 400 `InvalidSecurityGroup` (both correct, and the refusal is a no-op), with the
message built from the sentence the EC2 security-group lookup answers — "The security group
'<id>' does not exist" (EC2's `InvalidGroup.NotFound` wording) — inside that clause. The
recorded AWS message for the step is shape-asserted (`<shaped-not-asserted>`), so the corpus
discloses no text, and the shape it asserts still rejects what the emulator states. The
difference is therefore inside the sentence EC2 itself words for a group id a caller
REFERENCES, which this service must relay rather than re-word.

## Evidence
Fixture `elbclassic-subnet-attach-detach-and-security-group-reassignment`, step `task#15`.
Expected status 400 (matched), expected `$.ErrorResponse.Error.Message` `<shaped-not-asserted>`.
Each of the following was answered and replayed; the corpus accepted none, so none is the
recorded text (every one of them carries the clause above unless stated):

1. "The security group '<id>' does not exist" alone (the pre-fix behaviour).
2. The bare EC2 code token `InvalidGroup.NotFound` alone.
3. "The security group '<id>' does not exist", code `InvalidGroup.NotFound`.
4. "The security group '<id>' does not exist", code `InvalidSecurityGroupID.NotFound`.
5. "The securityGroup ID '<id>' does not exist", code `InvalidSecurityGroupID.NotFound`
   (the wording recorded for a referenced group id on EC2's own `CreateNetworkInterface`).
6. "The securityGroup ID '<id>' does not exist", code `InvalidGroup.NotFound`.
7. "The security group ID '<id>' does not exist", code `InvalidGroup.NotFound`.
8. "The security group '<id>' does not exist in VPC '<the load balancer's VPC>'", code
   `InvalidGroup.NotFound`.
9. The same VPC-scoped sentence with no clause at all.
10. The clause with no `; Proxy: null` tail.
11. "com.amazonaws.services.ec2.model.AmazonEC2Exception: " before sentence 3.
12. "Invalid security groups: [<id>]".
13. "One or more of the specified security groups do not exist: <id>".
14. "One or more of the specified security groups do not exist. (Service:
    AmazonElasticLoadBalancing; Status Code: 400; Error Code: InvalidSecurityGroup; Request ID:
    <id>; Proxy: null)".

For comparison, the subnet step's accepted message is "The subnet ID '<id>' does not exist
(Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound; Request ID: <that
call's id>; Proxy: null)", so the shape check is sensitive to the fixed text and the clause is
not what differs here.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Step `task#15`: call `ApplySecurityGroupsToLoadBalancer` on an existing VPC load balancer,
   naming a security group id the account holds no group for. The status (400) and the code
   (`InvalidSecurityGroup`) match; `$.ErrorResponse.Error.Message` fails the recorded shape.
3. Whoever owns this next needs the recorded sentence (or EC2's real wording for the call ELB
   makes when it associates security groups): the fix is then the EC2 security-group lookup's
   wording in
   `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/state/network.rs`
   (`resolve_security_groups`), which both ELB models relay — not the two handlers, which now
   state the shared rule.
