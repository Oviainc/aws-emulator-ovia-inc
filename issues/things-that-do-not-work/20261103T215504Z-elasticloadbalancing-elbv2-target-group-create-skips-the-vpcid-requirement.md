Issue-Source: trace-sweep-divergence
Issue-Trace: elbv2-ntraf-fargate-target-group-create-mistakes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbv2-ntraf-fargate-target-group-create-mistakes elbv2-alb5xx-traj-name-where-arn-required
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|CreateTargetGroup|4xx->2xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|CreateTargetGroup|4xx->2xx|status
Issue-Key: elasticloadbalancing|DescribeTargetGroups|2xx->4xx|$.DescribeTargetGroupsResponse
Issue-Key: elasticloadbalancing|DescribeTargetGroups|2xx->4xx|status
Issue-Commit: 2fe3407ad3a572d7c56bcafd231aa855d1c09ca1

# ELBv2 target group create skips the VpcId requirement and target group name lookup fails

> _Note: this issue was identified against the code base at commit `2fe3407ad3a572d7c56bcafd231aa855d1c09ca1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTargetGroup

## Description
This bundle carries 2 causes. They share one root fault in the target group resource: the emulator's CreateTargetGroup does not enforce the VpcId precondition that AWS applies for non-lambda target types, and the target group store it writes is not reachable by NAME, so a subsequent DescribeTargetGroups(Names=[...]) for a target group that exists fails.

Cause 1 of 2: CreateTargetGroup accepts a non-lambda target group with no VpcId and returns HTTP 200.
AWS: Smithy: CreateTargetGroup.Name is required; VpcId (shape VpcId) is optional in the model, and TargetType targets TargetTypeEnum [instance, ip, lambda, alb]. The VpcId-required-unless-lambda rule is therefore not expressible in the model — for the status, code and message this is recorded behavior only: the recording shows HTTP 400 with Error.Type 'Sender', Error.Code 'ValidationError' and Error.Message 'A VPC ID must be specified'. That message is entirely fixed text: it has no variable parts and names neither the submitted name nor the target type. AWS applies the rule when TargetType is instance or ip (the 'ip'/'instance' target types live in a VPC); the modeled errors for CreateTargetGroup (DuplicateTargetGroupName, InvalidConfigurationRequest, TooManyTags, TooManyTargetGroups) are not used for this fault.
Emulator: returns HTTP 200 and a fully formed CreateTargetGroupResponse, creating a target group with no VPC.
Why: a client that omits VpcId by mistake (a very common Fargate/ip-target scripting error) is told the target group was created. It then registers ip targets and attaches the group to a listener, and every downstream call succeeds locally but fails the moment the same template runs against AWS. The emulator also leaves vpc-less target groups in its state, so later reads return objects AWS can never produce.
Fix: CreateTargetGroup's validation — reject the request with ValidationError / 'A VPC ID must be specified' when VpcId is absent and TargetType is not lambda, before any state is written.

Cause 2 of 2: DescribeTargetGroups(Names=[...]) answers 400 for a target group that exists.
AWS: Smithy: DescribeTargetGroups input accepts LoadBalancerArn, TargetGroupArns, Names (TargetGroupNames) and pagination; modeled errors are LoadBalancerNotFoundException and TargetGroupNotFoundException, both 400. Looking a target group up by its Name is a first-class filter, so for a target group that exists AWS answers HTTP 200 with a DescribeTargetGroupsResult carrying that one TargetGroup, including its TargetGroupArn, TargetGroupName, Protocol, Port, VpcId, TargetType, IpAddressType, ProtocolVersion, the health-check members and an (empty) LoadBalancerArns list.
Emulator: answers HTTP 400 with no DescribeTargetGroupsResponse body at all, i.e. it treats the existing target group as not found when it is addressed by name rather than by ARN.
Why: name-based lookup is how a client resolves a target group's ARN when it only holds the name it chose — the standard first step of every attach/register/tag flow. Failing it makes the group unusable and makes a legitimately successful create look like it never happened; a reconcile loop keyed on this describe will recreate the target group and then hit DuplicateTargetGroupName.
Fix: DescribeTargetGroups' handler and the target group state index — the Names filter must resolve against the stored target group names and return 200 with the matching members, reserving TargetGroupNotFound for names that genuinely have no target group.

## Evidence
Cause 1 — fixture elbv2-ntraf-fargate-target-group-create-mistakes, step task#3, CreateTargetGroup. Expected status 400 with body {'Error': {'Type': 'Sender', 'Code': 'ValidationError', 'Message': 'A VPC ID must be specified'}, 'RequestId': <per-run>}; actual status 200 and no $.ErrorResponse at all. Cluster covers 3 steps of that fixture (task#3, task#4, task#5).

Cause 2 — fixture elbv2-alb5xx-traj-name-where-arn-required, step task#7, DescribeTargetGroups. Expected status 200 with $.DescribeTargetGroupsResponse.DescribeTargetGroupsResult.TargetGroups.member describing the existing target group (TargetGroupName, TargetGroupArn, Protocol HTTP, Port 8080, VpcId, TargetType ip, IpAddressType ipv4, ProtocolVersion HTTP1, HealthCheckEnabled true, HealthCheckProtocol HTTP, HealthCheckPath /, HealthCheckPort traffic-port, HealthCheckIntervalSeconds 30, HealthCheckTimeoutSeconds 5, HealthyThresholdCount 5, UnhealthyThresholdCount 2, Matcher.HttpCode 200, empty LoadBalancerArns); actual status 400 with $.DescribeTargetGroupsResponse missing entirely.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Cause 1: fixture elbv2-ntraf-fargate-target-group-create-mistakes, step task#3 — CreateTargetGroup for an ip/instance target type with VpcId omitted. Observe HTTP 200 instead of 400 ValidationError 'A VPC ID must be specified'.
3. Cause 2: fixture elbv2-alb5xx-traj-name-where-arn-required, step task#7 — after the target group has been created, call DescribeTargetGroups with Names=[<target group name>]. Observe HTTP 400 with no result body instead of HTTP 200 carrying the target group.
