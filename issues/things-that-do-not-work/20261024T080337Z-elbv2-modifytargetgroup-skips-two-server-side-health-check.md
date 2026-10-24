Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elbv2.asghc-target-group-health-check-port-out-of-range-rejected elbv2.asgdx-modify-tcp-probe-to-http-without-matcher-rejected
Issue-Key: ec2__elbv2.asghc-target-group-health-check-port-out-of-range-rejected|_error#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: elbv2.asgdx-modify-tcp-probe-to-http-without-matcher-rejected|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# ELBv2 ModifyTargetGroup skips two server-side health-check validations AWS enforces, so a refused modify is applied with 200

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyTargetGroup

## Description
TWO distinct root causes are bundled here; BOTH must be closed. ModifyTargetGroup is implemented (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/modify_target_group.rs is not a scaffold stub), and it answers 200 for two health-check configurations real AWS refuses at the service layer. Neither rule is expressible as a Smithy constraint, so neither is caught by the centralized validate step that runs before the handler — both have to be enforced in the handler itself.

NOTE on neighbouring open issues: 20261023T205437Z-elbv2-is-overly-permissive-and-under-populated-registertargets.md bundles ten CREATE-path/describe-path permissiveness causes (its Cause 4 mentions the HealthCheckPath rule "same rule in ModifyTargetGroup" in passing) and 20261023T205437Z-elbv2-ten-control-plane-operation-families-answer-501.md covers operations that answer 501. Neither carries the replay keys below, and neither states the two MODIFY-path rules in this issue.

Cause 1 of 2 — ModifyTargetGroup accepts a HealthCheckPort outside 1-65535.
AWS: Smithy ModifyTargetGroup.HealthCheckPort targets com.amazonaws.elasticloadbalancingv2#HealthCheckPort, a plain `string` with NO range constraint — so the numeric range rule is the SERVICE's, not the SDK's. AWS refuses the modify with HTTP 400 and error code ValidationError (the query-protocol generic client error; the operation's modeled errors are InvalidConfigurationRequestException/TargetGroupNotFoundException, neither of which is used for this), and the message is built from the request and states the rule: `Health check port '70000' must be within '1-65535' or 'traffic-port'` (recorded example of that rule's output). The refused modify must be a no-op: the stored HealthCheckPort stays at its previous value (4000 in the recording).
Emulator: the call SUCCEEDS — `AssertionError: the call succeeded; real AWS rejects it, so there is no error code` — so no error code was produced at all and the out-of-range port is written onto the persisted target group.
Why: a client that typo'd or computed a bad port gets a green control-plane answer and a target group whose probe can never run; the synchronous 400 ValidationError it would have corrected on never arrives, and read-back/drift-detection clients (CDK, Terraform, consoles) see a port AWS would never store. The accepted write also makes the "refused modify changes nothing" property unobservable.
Fix: HealthCheckPort validation in the ModifyTargetGroup handler — accept the literal `traffic-port` or a decimal integer in 1..=65535, otherwise refuse 400 ValidationError before any state mutation (the same rule belongs on CreateTargetGroup).
Traces: ec2__elbv2.asghc-target-group-health-check-port-out-of-range-rejected.

Cause 2 of 2 — ModifyTargetGroup accepts a layer-7 (HTTP) health check with a path but no Matcher.
AWS: moving a TCP-probed target group to HealthCheckProtocol=HTTP with a HealthCheckPath but no Matcher is refused with HTTP 400 ValidationError; the message states the rule: `Path and return code are required for layer 7 health checks.` (recorded example). The asymmetry is deliberate on AWS's side: CreateTargetGroup fills the Matcher default (HttpCode 200) for an HTTP check, the MODIFY path does not, and the refused modify leaves the probe exactly as it was (HealthCheckProtocol=TCP, HealthCheckPort=traffic-port, no HealthCheckPath). Smithy models Matcher as an optional structure (members HttpCode/GrpcCode) with no requiredness tie to HealthCheckProtocol, so this cross-field rule is server-side only.
Emulator: the call SUCCEEDS — `AssertionError: the call succeeded; real AWS rejects it, so there is no error code` — so the HTTP probe is applied with no return-code matcher.
Why: the emulator lets a target group reach a state AWS cannot produce (a layer-7 probe with no success criterion); every later health evaluation and every read-back of that group diverges, and the client never sees the 400 that would have told it to supply the matcher. It also hides the create/modify default asymmetry a customer's update path depends on.
Fix: cross-field validation in the ModifyTargetGroup handler — when the resulting HealthCheckProtocol is HTTP/HTTPS, require both a path and a matcher (do NOT silently inherit the create-path default), refuse 400 ValidationError otherwise, and guarantee the refusal mutates nothing.
Traces: elbv2.asgdx-modify-tcp-probe-to-http-without-matcher-rejected.

## Evidence
Cause 1: expected HTTP 400 ValidationError with a request-derived message of the form "Health check port '<value>' must be within '1-65535' or 'traffic-port'" and HealthCheckPort still '4000' on DescribeTargetGroups; actual — modify_target_group(TargetGroupArn=arn, HealthCheckPort="70000") returned success, check failed at check_ec2__elbv2_asghc_target_group_health_check_port_out_of_range_rejected.py line 50 with "AssertionError: the call succeeded; real AWS rejects it, so there is no error code".
Cause 2: expected HTTP 400 ValidationError with message "Path and return code are required for layer 7 health checks." and the probe still (TCP, 'traffic-port', None); actual — modify_target_group(TargetGroupArn=arn, HealthCheckProtocol='HTTP', HealthCheckPath=..., no Matcher) returned success, check failed at check_elbv2_asgdx_modify_tcp_probe_to_http_without_matcher_rejected.py line 36 with "AssertionError: the call succeeded; real AWS rejects it, so there is no error code".
Smithy cited: ModifyTargetGroup input {TargetGroupArn required, HealthCheckPort -> HealthCheckPort (string, no range), HealthCheckPath -> Path @length 1..1024, Matcher optional}; errors InvalidConfigurationRequestException (awsQueryError InvalidConfigurationRequest, 400), TargetGroupNotFoundException (awsQueryError TargetGroupNotFound, 400).

## Steps to Reproduce
1. Create a VPC and one subnet (any CIDR, e.g. 172.25.0.0/16 with a /23 subnet).
2. CreateTargetGroup(Name=<unique>, Protocol='TCP', Port=44300, VpcId=<vpc>, TargetType='instance', HealthCheckEnabled=true, HealthCheckProtocol='HTTP', HealthCheckPort='4000', HealthCheckPath='/health-check', HealthCheckIntervalSeconds=20, HealthCheckTimeoutSeconds=10, HealthyThresholdCount=2, UnhealthyThresholdCount=10).
3. Cause 1: ModifyTargetGroup(TargetGroupArn=<arn>, HealthCheckPort='70000') -> emulator returns 200; AWS returns 400 ValidationError and leaves HealthCheckPort='4000'. Then DescribeTargetGroups(TargetGroupArns=[arn]) and observe the stored port.
4. Cause 2: CreateTargetGroup(Name=<unique>, Protocol='TCP', Port=<traffic port>, VpcId=<vpc>, TargetType='instance') with no health-check overrides (default TCP probe), then ModifyTargetGroup(TargetGroupArn=<arn>, HealthCheckProtocol='HTTP', HealthCheckPath='/<path>', HealthCheckPort=..., and NO Matcher) -> emulator returns 200; AWS returns 400 ValidationError 'Path and return code are required for layer 7 health checks.' and DescribeTargetGroups still reports ('TCP', 'traffic-port', no HealthCheckPath).
5. Repeat the same ModifyTargetGroup WITH Matcher={'HttpCode':'200-399'} -> this one must succeed, keep the ARN, and round-trip every health-check member plus Protocol/Port unchanged.
