Issue-Source: trace-sweep-divergence
Issue-Trace: elasticloadbalancing-alb-lifecycle-register-targets
Issue-Step: setup#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticloadbalancing-alb-lifecycle-register-targets elasticloadbalancing-delete-listener-idempotent-redelete elasticloadbalancing-lambda-target-group-lifecycle elasticloadbalancing-listener-rule-path-based-routing elasticloadbalancing-modify-listener-port-updates elasticloadbalancing-rule-condition-values-mirroring elasticloadbalancing-tag-value-unicode-round-trip elbv2-deregister-nonexistent-target-is-idempotent elbv2-describe-listeners-filtered-by-load-balancer elbv2-describe-target-health-unregistered-target-unused elbv2-empty-string-vs-absent-tag-value-distinguished elbv2-https-listener-with-cert elbv2-listener-attribute-and-certificate-error-paths elbv2-modify-listener-inplace-update-and-notfound elbv2-op-contract-describe-listeners elbv2-op-contract-describe-target-group-attributes elbv2-tag-management-lifecycle elbv2-tag-value-unicode-and-special-characters-round-trip elbv2-target-group-and-target-registration elbv2-target-group-deregistration-delay-attribute-round-trip elbv2-target-group-deregistration-delay-numeric-precision-round-trip elbv2-target-group-lambda-lifecycle-with-tags elbv2env-listener-attributes-targets-and-rule-pagination elbv2err-listener-create-rejections elbv2err-register-targets-type-mismatch elasticloadbalancing-create-target-group-health-check-defaults elbv2-create-target-group-basic elasticloadbalancing-elbv2-distinct-error-conditions elbv2-rule-target-group-load-balancer-not-found-errors
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|CreateTargetGroup|2xx->4xx|status
Issue-Key: elasticloadbalancing|CreateTargetGroup|2xx->4xx|$.CreateTargetGroupResponse
Issue-Key: elasticloadbalancing|CreateTargetGroup|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|CreateTargetGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|CreateTargetGroup|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DescribeTargetGroups|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeTargetGroups|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeTargetGroups|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|ModifyTargetGroup|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|ModifyTargetGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|ModifyTargetGroup|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|ModifyTargetGroupAttributes|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|ModifyTargetGroupAttributes|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|ModifyTargetGroupAttributes|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DescribeTargetGroupAttributes|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeTargetGroupAttributes|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeTargetGroupAttributes|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|RegisterTargets|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|RegisterTargets|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|RegisterTargets|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DeregisterTargets|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DeregisterTargets|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DeregisterTargets|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# elasticloadbalancing ELBv2 target group and target registration actions answer 400 InvalidAction

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTargetGroup

## Description
10 causes, one mechanism: the shared 'elasticloadbalancing' front door is the Classic ELB crate, which serves only Version=2012-06-01 and refuses every 2015-12-01 envelope with 400 InvalidAction 'Could not find operation <Action> for version <Version>' (services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/service/api_version.rs); the ELBv2 model answers only under its private route key 'elasticloadbalancingv2'. Related OPEN issues that do NOT cover these fixture steps and carry no wire replay key: 20261023T025808Z-elbv2-awsquery-action-table-at-api-version-2015.md, 20261023T025808Z-autoscaling-ec2-elbv2-auto-scaling-ec2-elbv2-integration-blocked-elbv2-createtargetgroup.md, 20261023T025808Z-ec2-elbv2-the-whole-ec2-elbv2-ip-target-surface-is.md. Each cause needs the version dispatch fixed and then that operation's own handler, defaulting, validation and serializer.

Cause 1 of 10: CreateTargetGroup success path.
AWS: Smithy CreateTargetGroup(Name required, Protocol, Port 1..65535, VpcId, health-check members, TargetType, Tags) -> {TargetGroups}; a valid create is 200 with TargetGroupArn and the resolved configuration.
Emulator: 400 InvalidAction.
Why: no target group can be created, so every registration, health and rule-action workflow is blocked at setup.
Fix: version dispatch, then CreateTargetGroup handler/state/serializer.
Traces: elasticloadbalancing-alb-lifecycle-register-targets#setup#2 (cluster of 29 setup steps).

Cause 2 of 10: CreateTargetGroup health-check defaulting.
AWS: omitted health-check members are defaulted and echoed back — recorded 200 shows HealthCheckEnabled true, HealthCheckProtocol HTTP, HealthCheckPath '/', HealthCheckPort 'traffic-port', HealthCheckIntervalSeconds 30, HealthCheckTimeoutSeconds 5, HealthyThresholdCount 5, UnhealthyThresholdCount 2, Matcher.HttpCode '200', IpAddressType ipv4, ProtocolVersion HTTP1 (Smithy ranges: interval 5..300, timeout 2..120, thresholds 2..10).
Emulator: 400 InvalidAction, no CreateTargetGroupResponse.
Why: a client that relies on server-side defaults gets no configuration at all.
Fix: version dispatch, then CreateTargetGroup's default resolution.
Traces: elasticloadbalancing-create-target-group-health-check-defaults#task#2 (cluster of 4).

Cause 3 of 10: CreateTargetGroup name-length validation.
AWS: an over-long Name is refused 400 with Code 'ValidationError' and a request-derived message naming the value and the limit (example "Target group name 'sim2real-probe-elbv2-name-far-too-long' cannot be longer than '32' characters").
Emulator: 400 Code 'InvalidAction'.
Why: input validation never runs, so a client cannot learn which member it got wrong.
Fix: version dispatch, then CreateTargetGroup's Name validation.
Traces: elasticloadbalancing-elbv2-distinct-error-conditions#task#4 (cluster of 2).

Cause 4 of 10: CreateTargetGroup duplicate-name refusal.
AWS: Smithy declares DuplicateTargetGroupNameException, awsQueryError 'DuplicateTargetGroupName', HTTP 400 (example "A target group with the same name 'sim2real-probe-elbv2-tg-f4b70f1c' exists, but with different settings").
Emulator: 400 Code 'InvalidAction'.
Why: conflict handling keyed on DuplicateTargetGroupName never fires.
Fix: version dispatch, then CreateTargetGroup's name-uniqueness check.
Traces: elbv2-target-group-and-target-registration#task#7.

Cause 5 of 10: DescribeTargetGroups for an unknown target group.
AWS: Smithy declares TargetGroupNotFoundException / 'TargetGroupNotFound' / 400 (example 'One or more target groups not found').
Emulator: 400 Code 'InvalidAction'.
Why: discovery and its not-found branch are unreachable.
Fix: version dispatch, then DescribeTargetGroups handler.
Traces: elbv2-rule-target-group-load-balancer-not-found-errors#task#8.

Cause 6 of 10: ModifyTargetGroup on an absent target group.
AWS: TargetGroupNotFound / 400, message naming the ARN.
Emulator: 400 InvalidAction.
Why: health-check reconfiguration is unreachable.
Fix: version dispatch, then ModifyTargetGroup handler.
Traces: elbv2-rule-target-group-load-balancer-not-found-errors#task#6.

Cause 7 of 10: ModifyTargetGroupAttributes on an absent target group.
AWS: TargetGroupNotFound / 400.
Emulator: 400 InvalidAction.
Why: attribute writes (deregistration delay, stickiness) are unreachable.
Fix: version dispatch, then ModifyTargetGroupAttributes handler.
Traces: elbv2-rule-target-group-load-balancer-not-found-errors#task#7.

Cause 8 of 10: DescribeTargetGroupAttributes on an absent target group.
AWS: TargetGroupNotFound / 400, message naming the ARN.
Emulator: 400 InvalidAction.
Why: attribute reads are unreachable.
Fix: version dispatch, then DescribeTargetGroupAttributes handler.
Traces: elbv2-op-contract-describe-target-group-attributes#task#3 (cluster of 2).

Cause 9 of 10: RegisterTargets on an absent target group.
AWS: TargetGroupNotFound / 400 (alongside InvalidTarget / TooManyTargets).
Emulator: 400 InvalidAction.
Why: no target can ever be registered, so target health is never exercised.
Fix: version dispatch, then RegisterTargets handler.
Traces: elbv2-rule-target-group-load-balancer-not-found-errors#task#2.

Cause 10 of 10: DeregisterTargets on an absent target group.
AWS: TargetGroupNotFound / 400.
Emulator: 400 InvalidAction.
Why: deregistration and its idempotency semantics are unreachable.
Fix: version dispatch, then DeregisterTargets handler.
Traces: elbv2-rule-target-group-load-balancer-not-found-errors#task#3.

## Evidence
All actual responses are HTTP 400 with $.ErrorResponse.Error.Code 'InvalidAction' and Message 'Could not find operation <Action> for version 2015-12-01' (and no $.ErrorResponse.RequestId where one was expected).
- elasticloadbalancing-alb-lifecycle-register-targets#setup#2 CreateTargetGroup: expected 200, got 400 (29 steps in cluster).
- elasticloadbalancing-create-target-group-health-check-defaults#task#2: expected 200 + CreateTargetGroupResult.TargetGroups.member{HealthCheckEnabled 'true', HealthCheckProtocol 'HTTP', HealthCheckPath '/', HealthCheckPort 'traffic-port', HealthCheckIntervalSeconds '30', HealthCheckTimeoutSeconds '5', HealthyThresholdCount '5', UnhealthyThresholdCount '2', Matcher.HttpCode '200'}; got 400.
- elasticloadbalancing-elbv2-distinct-error-conditions#task#4: expected Code 'ValidationError' with message "Target group name '...' cannot be longer than '32' characters"; got 'InvalidAction'.
- elbv2-target-group-and-target-registration#task#7: expected 'DuplicateTargetGroupName'; got 'InvalidAction'.
- elbv2-rule-target-group-load-balancer-not-found-errors#task#8/#task#6/#task#7/#task#2/#task#3 and elbv2-op-contract-describe-target-group-attributes#task#3: expected 'TargetGroupNotFound' for DescribeTargetGroups / ModifyTargetGroup / ModifyTargetGroupAttributes / RegisterTargets / DeregisterTargets / DescribeTargetGroupAttributes; got 'InvalidAction'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Signing for service 'elasticloadbalancing', POST 'Action=CreateTargetGroup&Version=2015-12-01&Name=probe-tg&Protocol=HTTP&Port=80&VpcId=<vpc>&TargetType=instance'. Observe 400 InvalidAction instead of 200 + TargetGroups with defaulted health-check members.
3. Repeat with an over-long Name (expect ValidationError), with a duplicate Name (expect DuplicateTargetGroupName), and with Action = DescribeTargetGroups / ModifyTargetGroup / ModifyTargetGroupAttributes / DescribeTargetGroupAttributes / RegisterTargets / DeregisterTargets against a fabricated target-group ARN (expect TargetGroupNotFound).
4. Re-replay the fixture steps named in issue_step and trace_ids.
