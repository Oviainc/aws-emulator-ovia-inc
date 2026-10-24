Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elbv2.ip-target-from-a-public-range-rejected ec2__elbv2.ip-target-in-the-vpc-cidr-but-outside-every-subnet-rejected ec2__elbv2.ecsnt-ip-target-outside-the-vpc-rejected elbv2.register-targets-duplicate-id-conflicting-port-rejected elbv2.register-targets-exceeding-target-group-quota-rejected elbv2.create-target-group-malformed-health-check-path-rejected elbv2.create-listener-unsupported-protocol-version-rejected ec2__elbv2.ecsnt-ip-target-group-applies-health-check-and-attribute-defaults elbv2.asgdx-tcp-group-http-side-port-health-check-round-trips ec2__elbv2__opensearch.vpc-domain-and-internal-alb-with-a-security-group-from-another-vpc-rejected elbv2.alb-listener-target-group-lifecycle ec2__elbv2.albos-ip-target-availability-zone-resolves-from-the-containing-subnet elbv2.describe-target-health-unknown-group-fails elbv2.asgdx-unhealthy-threshold-above-ten-rejected elbv2.create-load-balancer-invalid-scheme-rejected
Issue-Key: ec2__elbv2.ip-target-from-a-public-range-rejected|_error#5|AssertionError|raise AssertionError(f"the call was expected to be REFUSED but it succeeded: {result!r}")
Issue-Key: ec2__elbv2.ip-target-in-the-vpc-cidr-but-outside-every-subnet-rejected|_error#6|AssertionError|raise AssertionError(f"the call was expected to be REFUSED but it succeeded: {result!r}")
Issue-Key: ec2__elbv2.ecsnt-ip-target-outside-the-vpc-rejected|_error#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: elbv2.register-targets-duplicate-id-conflicting-port-rejected|_error#8|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: elbv2.register-targets-exceeding-target-group-quota-rejected|_error_code#25|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: elbv2.create-target-group-malformed-health-check-path-rejected|_error#4|AssertionError|raise AssertionError("expected the call to be refused, but it succeeded")
Issue-Key: elbv2.create-listener-unsupported-protocol-version-rejected|run#6|AssertionError|raise AssertionError(
Issue-Key: ec2__elbv2.ecsnt-ip-target-group-applies-health-check-and-attribute-defaults|run#7|AssertionError|assert {k: got.get(k) for k in want} == want, f"target group: {got!r}"
Issue-Key: elbv2.asgdx-tcp-group-http-side-port-health-check-round-trips|run#10|AssertionError|assert tg.get("Matcher") == {"HttpCode": "200-399"}, f"HTTP health-check matcher default is {tg.get('Matcher')!r}"
Issue-Key: ec2__elbv2__opensearch.vpc-domain-and-internal-alb-with-a-security-group-from-another-vpc-rejected|_error#5|AssertionError|raise AssertionError(f"the call was expected to be REFUSED but it succeeded: {result!r}")
Issue-Key: elbv2.alb-listener-target-group-lifecycle|run#23|AssertionError|assert got == want, f"DescribeLoadBalancers (name, type, scheme, vpc) expected {want!r}, got {got!r}"
Issue-Key: ec2__elbv2.albos-ip-target-availability-zone-resolves-from-the-containing-subnet|run#12|AssertionError|assert got == expect, f"target zones read {got!r}, the containing subnets say {expect!r}"
Issue-Key: elbv2.describe-target-health-unknown-group-fails|run#14|AssertionError|assert message == f"Target groups '{fake_arn}' not found", f"MESSAGE={message!r}"
Issue-Key: elbv2.asgdx-unhealthy-threshold-above-ten-rejected|run#13|AssertionError|assert left_msg == want_left, f"the lookup of {bad!r} said {left_msg!r}, expected {want_left!r}"
Issue-Key: elbv2.create-load-balancer-invalid-scheme-rejected|run#12|AssertionError|assert not_found_message == expected_not_found, (
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# ELBv2 is overly permissive and under-populated: RegisterTargets/CreateTargetGroup/CreateLoadBalancer accept input AWS refuses, describes omit VpcId and target AZ, and not-found messages use the wrong form

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTargets

## Description
TEN distinct root causes are bundled here; all ten must be closed. Unlike the 501-stub issue, these operations DO answer — they answer wrongly.

Cause 1 of 10 — RegisterTargets accepts IP targets AWS refuses.
AWS: Smithy RegisterTargets(TargetGroupArn, Targets required) models InvalidTargetException (awsQueryError InvalidTarget, httpError 400). For a TargetType=ip group, AWS accepts only addresses inside the target group's VPC (and, for an ip group, inside a subnet of that VPC); a publicly routable address, an address outside the VPC CIDR, and an address inside the VPC CIDR but in no subnet are all refused 400 InvalidTarget.
Emulator: all three register successfully — HTTP 200 with an empty RegisterTargetsOutput.
Why: a misconfigured target silently becomes a registered target that can never pass a health check; the client gets a green control-plane answer and a black hole in the data plane, instead of a synchronous InvalidTarget it can correct.
Fix: target validation in the RegisterTargets handler (resolve the group's VpcId and its subnets' CIDRs before accepting an ip target).
Traces: ec2__elbv2.ip-target-from-a-public-range-rejected, ec2__elbv2.ip-target-in-the-vpc-cidr-but-outside-every-subnet-rejected, ec2__elbv2.ecsnt-ip-target-outside-the-vpc-rejected.

Cause 2 of 10 — RegisterTargets accepts the same target id twice with conflicting ports.
AWS: refuses the request (400, InvalidTarget class) rather than storing two conflicting registrations for one id.
Emulator: the call SUCCEEDED and returned None (200).
Why: the target group ends up in a state AWS cannot produce, so deregistration and health reporting are ambiguous.
Fix: duplicate-id/port conflict detection in the RegisterTargets handler.
Traces: elbv2.register-targets-duplicate-id-conflicting-port-rejected.

Cause 3 of 10 — RegisterTargets ignores the per-target-group target quota.
AWS: models TooManyTargetsException (awsQueryError TooManyTargets, 400) and refuses a registration that would exceed the account's targets-per-target-group limit.
Emulator: the over-quota registration returns HTTP 200.
Why: quota-exhaustion handling (the retry/backoff branch of every autoscaling client) is never exercised and capacity planning signals vanish.
Fix: quota accounting in the RegisterTargets handler, fed by the same limit catalogue DescribeAccountLimits should serve.
Traces: elbv2.register-targets-exceeding-target-group-quota-rejected.

Cause 4 of 10 — CreateTargetGroup accepts a malformed HealthCheckPath.
AWS: refuses with 400 ValidationError; the message is built from the request and states the rule, e.g. "Health check path 'missing-leading-slash' must begin with a '/' character and can only contain printable ASCII characters, without spaces". Smithy types HealthCheckPath as Path @length 1..1024 — the leading-slash/printable-ASCII rule is server-side.
Emulator: the create succeeds and a target group exists with an unusable probe path.
Why: the probe silently never matches, so every target is unhealthy with no diagnostic; AWS would have refused the create.
Fix: HealthCheckPath validation in the CreateTargetGroup handler (same rule in ModifyTargetGroup).
Traces: elbv2.create-target-group-malformed-health-check-path-rejected.

Cause 5 of 10 — CreateTargetGroup accepts ProtocolVersion=HTTP2 on a TCP group.
AWS: ProtocolVersion applies only to HTTP/HTTPS groups; a TCP group with ProtocolVersion=HTTP2 is refused with InvalidConfigurationRequestException (awsQueryError InvalidConfigurationRequest, 400) — a modeled error of CreateTargetGroup.
Emulator: the call succeeded and created the target group (ARN returned).
Why: an impossible protocol combination is persisted; listeners built on it diverge from AWS from then on.
Fix: protocol/ProtocolVersion cross-validation in the CreateTargetGroup handler.
Traces: elbv2.create-listener-unsupported-protocol-version-rejected.

Cause 6 of 10 — target-group health-check defaults are wrong/absent.
AWS: for an HTTP target group created with only Protocol/Port/VpcId/TargetType/HealthCheckProtocol/HealthCheckPath, DescribeTargetGroups fills HealthCheckPort='traffic-port', HealthCheckIntervalSeconds=30, HealthCheckTimeoutSeconds=5, HealthyThresholdCount=5, UnhealthyThresholdCount=2, Matcher={'HttpCode':'200'} and ProtocolVersion='HTTP1' (model: HealthCheckTimeoutSeconds @range 2..120, thresholds @range 2..10).
Emulator: returns HealthCheckTimeoutSeconds=6 and omits ProtocolVersion entirely; the HTTP matcher default is likewise not filled for a TCP group whose probe is HTTP.
Why: clients that read back defaults (CDK/Terraform drift detection, consoles) see a configuration the account never requested and AWS would never return, and a 6 s timeout changes probe behaviour.
Fix: the default-filling in the CreateTargetGroup handler / target-group serializer.
Traces: ec2__elbv2.ecsnt-ip-target-group-applies-health-check-and-attribute-defaults, elbv2.asgdx-tcp-group-http-side-port-health-check-round-trips.

Cause 7 of 10 — CreateLoadBalancer accepts a security group from a different VPC.
AWS: refuses with InvalidConfigurationRequest/InvalidSecurityGroup (400) — the security groups must belong to the VPC the subnets are in.
Emulator: the call succeeded; DescribeLoadBalancers then lists the internal ALB with the foreign sg-... attached (State=provisioning).
Why: a cross-VPC security group is unenforceable, so an emulated deployment "works" where the real one is rejected at create time.
Fix: security-group/subnet VPC consistency validation in the CreateLoadBalancer handler. (The open issue 20261023T025808Z-ec2-elbv2-opensearch-opensearch-alb-diagnosis-scenarios-blocked-cfn-stack-outputs.md records the older symptom — the same call answered InvalidAction — and does not carry this replay key.)
Traces: ec2__elbv2__opensearch.vpc-domain-and-internal-alb-with-a-security-group-from-another-vpc-rejected.

Cause 8 of 10 — DescribeLoadBalancers omits VpcId.
AWS: every DescribeLoadBalancers entry carries the VpcId the load balancer's subnets belong to.
Emulator: VpcId comes back null — "expected ('sim2real-albl-...', 'application', 'internal', 'vpc-...'), got (..., None)".
Why: any client that resolves a load balancer's VPC from a describe (security-group creation, ECS/ASG wiring, CFN importers) gets None and fails or guesses.
Fix: populate VpcId in the load-balancer state/serializer from the subnets given to CreateLoadBalancer.
Traces: elbv2.alb-listener-target-group-lifecycle.

Cause 9 of 10 — DescribeTargetHealth reports no AvailabilityZone for ip targets.
AWS: for an ip target, the TargetHealthDescription's Target.AvailabilityZone is the zone of the subnet containing that address.
Emulator: both targets report AvailabilityZone=None while the containing subnets are us-west-2a / us-west-2b.
Why: zonal health and cross-zone reasoning (the common ALB troubleshooting path) is impossible.
Fix: resolve the containing subnet's AZ when serializing target health / registering an ip target.
Traces: ec2__elbv2.albos-ip-target-availability-zone-resolves-from-the-containing-subnet.

Cause 10 of 10 — not-found error messages use the singular form for the plural lookup APIs.
AWS: the message is built from the request. DescribeTargetGroups(Names=[...]) -> TargetGroupNotFound with "One or more target groups not found"; DescribeTargetHealth(TargetGroupArn=arn) -> TargetGroupNotFound with "Target groups '<arn>' not found"; DescribeLoadBalancers(Names=[name]) -> LoadBalancerNotFound with "Load balancers '[<name>]' not found".
Emulator: always the single-resource form — "Target group '<x>' not found", "Load balancer '<name>' not found". The HTTP status (400) and the error codes (TargetGroupNotFound / LoadBalancerNotFound) are correct; only the request-derived message form diverges.
Why: clients and test harnesses that match on these exact AWS-generated strings (and operators reading them) see a message AWS never emits for a list-shaped lookup. Lowest-severity cause in this bundle.
Fix: the not-found message construction in the describe handlers (choose the list form when the lookup was by a list member).
Traces: elbv2.describe-target-health-unknown-group-fails, elbv2.asgdx-unhealthy-threshold-above-ten-rejected, elbv2.create-load-balancer-invalid-scheme-rejected.

## Evidence
Cause 1: expected 400 InvalidTarget; actual HTTP 200 — "the call was expected to be REFUSED but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}".
Cause 2: expected rejection; actual "real AWS rejects this call, but it SUCCEEDED and returned None".
Cause 3: expected 400 TooManyTargets; actual "real AWS rejects this call, but it SUCCEEDED and returned {... 'HTTPStatusCode': 200 ...}".
Cause 4: expected 400 ValidationError "Health check path 'missing-leading-slash' must begin with a '/' character ..."; actual "expected the call to be refused, but it succeeded".
Cause 5: expected 400 InvalidConfigurationRequest; actual "the call succeeded and created arn:aws:elasticloadbalancing:...:targetgroup/sim2real-behav-tg-b-.../...".
Cause 6: expected HealthCheckTimeoutSeconds=5 and ProtocolVersion='HTTP1'; actual DescribeTargetGroups returned HealthCheckTimeoutSeconds=6 with no ProtocolVersion member.
Cause 7: expected 400 (InvalidConfigurationRequest/InvalidSecurityGroup); actual 200 and DescribeLoadBalancers lists the ALB with SecurityGroups=['sg-...'] from the other VPC.
Cause 8: expected VpcId='vpc-...'; actual None.
Cause 9: expected {'10.80.4.37': 'us-west-2b', '10.80.8.112': 'us-west-2a'}; actual {'10.80.4.37': None, '10.80.8.112': None}.
Cause 10: expected 'One or more target groups not found' / "Target groups '<arn>' not found" / "Load balancers '[<name>]' not found"; actual "Target group '<x>' not found" / "Load balancer '<name>' not found" (code and 400 status correct).

## Steps to Reproduce
1. Create a VPC (e.g. 10.80.0.0/16) with two subnets in different AZs.
2. Cause 1: CreateTargetGroup(TargetType='ip', Protocol='HTTP', Port=80, VpcId=vpc) then RegisterTargets with (a) a public address, (b) an address in the VPC CIDR but in no subnet, (c) an address outside the VPC CIDR -> each returns 200 instead of 400 InvalidTarget.
3. Cause 2: RegisterTargets with the same Id twice at different Ports -> 200.
4. Cause 3: RegisterTargets with more targets than the per-group quota -> 200.
5. Cause 4: CreateTargetGroup(..., HealthCheckProtocol='HTTP', HealthCheckPath='missing-leading-slash') -> 200.
6. Cause 5: CreateTargetGroup(Protocol='TCP', ProtocolVersion='HTTP2') -> 200.
7. Cause 6: CreateTargetGroup with only Protocol/Port/VpcId/TargetType/HealthCheckProtocol/HealthCheckPath, then DescribeTargetGroups -> HealthCheckTimeoutSeconds=6, ProtocolVersion absent.
8. Cause 7: CreateLoadBalancer(Subnets=<vpc A subnets>, SecurityGroups=[<sg in vpc B>]) -> 200.
9. Cause 8: DescribeLoadBalancers on it -> VpcId null.
10. Cause 9: register two ip targets in different subnets, DescribeTargetHealth -> Target.AvailabilityZone null.
11. Cause 10: DescribeTargetGroups(Names=['nope']), DescribeTargetHealth(TargetGroupArn='arn:...:targetgroup/ghost-tg/0123456789abcdef'), DescribeLoadBalancers(Names=['nope']) -> compare the messages with the AWS forms above.
