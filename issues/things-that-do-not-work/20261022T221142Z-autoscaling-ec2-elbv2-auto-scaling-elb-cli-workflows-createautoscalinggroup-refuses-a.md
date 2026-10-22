Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.cli-target-group-attach-fails-with-invalid-arn autoscaling__ec2__elbv2.cli-asg-attach-load-balancer-target-groups-command autoscaling__ec2__elbv2.cli-create-asg-with-target-group-and-verify-ec2-registration
Issue-Key: autoscaling__ec2__elbv2.cli-target-group-attach-fails-with-invalid-arn|_run_shell#7|AssertionError|raise AssertionError(
Issue-Key: autoscaling__ec2__elbv2.cli-asg-attach-load-balancer-target-groups-command|_create_target_group#6|ResponseParserError|resp = elbv2.create_target_group(
Issue-Key: autoscaling__ec2__elbv2.cli-create-asg-with-target-group-and-verify-ec2-registration|_create_target_group#6|ResponseParserError|resp = elbv2.create_target_group(
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# Auto Scaling + ELB CLI workflows: CreateAutoScalingGroup refuses a VPCZoneIdentifier-only group with 501 NotImplemented, and every 'elasticloadbalancing' request is refused with a JSON AmbiguousService body (2 causes)

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
TWO distinct root causes on Auto Scaling / ELB cross-service paths that the emulator answers (it is up and serving — 6 checks in this suite pass). Neither of the three grounding checks was marked ALREADY FILED or OWNED by the replay report, so a fixer must close BOTH causes before submitting.

Cause 1 of 2: CreateAutoScalingGroup refuses a group that names only VPCZoneIdentifier (no AvailabilityZones) with a 501 NotImplemented.
AWS: in CreateAutoScalingGroup's input (Smithy: CreateAutoScalingGroupType, model services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json) only AutoScalingGroupName, MinSize and MaxSize are required; AvailabilityZones (target AvailabilityZones) and VPCZoneIdentifier (XmlStringMaxLen5000) are both optional, and a request that supplies VPCZoneIdentifier alone is accepted — Auto Scaling resolves each named subnet's Availability Zone itself (through EC2) and the created group reports those derived AvailabilityZones. The operation's only modeled errors are AlreadyExistsFault (awsQueryError AlreadyExists, 400), LimitExceededFault (LimitExceeded, 400), ResourceContentionFault (ResourceContention, 500) and ServiceLinkedRoleFailure (500); there is no NotImplemented error and no 501 on this operation.
Emulator: the create is rejected outright with error code NotImplemented (recorded example message: "launching instances for a group that names only VPCZoneIdentifier needs each subnet's Availability Zone, which this emulator does not hold: pass AvailabilityZones as well"), so no group is created at all; the AWS CLI exits rc=254. The emulator is asking the caller to supply a value AWS derives server-side.
Why: the VPCZoneIdentifier-only form is the normal, documented way to create a VPC-backed group (it is what the CLI, CloudFormation and CDK emit), so a client that never sets AvailabilityZones cannot create a group at all; the failure also arrives as an unmodeled, non-retryable error code that no SDK error handler can classify, and every downstream assertion (attach target groups, scale, terminate) is blocked.
Fix: the CreateAutoScalingGroup handler in the autoscaling crate (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/create_auto_scaling_group.rs) plus the group state it writes — resolve each subnet id in VPCZoneIdentifier against the emulated EC2 subnet state to derive the group's Availability Zones instead of refusing, and reserve the modeled error set for genuine failures.
Traces: autoscaling__ec2__elbv2.cli-target-group-attach-fails-with-invalid-arn

Cause 2 of 2: two emulated services both claim the 'elasticloadbalancing' endpoint prefix, so every ELB/ELBv2 request made by these CLI workflows is refused — and refused with a JSON body to a Query-protocol client.
AWS: classic Elastic Load Balancing (awsQuery, 2012-06-01) and Elastic Load Balancing v2 (awsQuery, 2015-12-01) share the endpoint prefix 'elasticloadbalancing' (smithy_query service_info on services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/smithy-model/com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.json: awsService.endpointPrefix='elasticloadbalancing', sigv4Name='elasticloadbalancing', protocol awsQuery, version 2015-12-01; the v7/classic crate declares the same prefix for 2012-06-01) and are disambiguated by the request's Action/Version, so an elbv2 CreateTargetGroup / DescribeTargetHealth call returns its normal 200 XML envelope, and any failure is an XML ErrorResponse with a modeled error code.
Emulator: the request is refused with the body {"__type":"AmbiguousService","message":"2 emulated services all claim requests for 'elasticloadbalancing': elasticloadbalancing, elasticloadbalancing; the request is rejected rather than served by an arbitrary one"} — a JSON error delivered to a Query/XML client, which cannot parse it (botocore raises ResponseParserError: 'invalid XML received'); 'AmbiguousService' is not a modeled error code of either load-balancing service.
Why: no client can use the load-balancing APIs through the emulator while both crates are registered; the failure surfaces as a parser crash rather than an API error, so it is neither retry-classifiable nor reportable, and it takes down every ASG/ECS + ALB workflow that touches a target group.
Fix: request routing / service registration in the dispatcher — key the two load-balancing crates by their distinct wire identity (awsQuery Version plus Action) rather than by the shared endpoint prefix so a request resolves to exactly one service, and serialize anything the dispatcher still cannot resolve in the protocol of the addressed service (XML ErrorResponse for awsQuery), never as a JSON __type body.
NOTE: the same root cause is already described as 'Cause 3 of 4' of the open issue 20261022T175547Z-autoscaling-auto-scaling-4-fidelity-divergences-cloudformation-asg-never.md, which is keyed only to autoscaling__ecs__elbv2.deploy-ecs-service-with-alb-target-group-cfn. The two checks below were NOT marked OWNED by that issue in this replay, so they are filed here to carry their replay keys; closing the routing defect closes both.
Traces: autoscaling__ec2__elbv2.cli-asg-attach-load-balancer-target-groups-command, autoscaling__ec2__elbv2.cli-create-asg-with-target-group-and-verify-ec2-registration

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (6/388 checks passed, so the emulator is serving).

Cause 1 — autoscaling__ec2__elbv2.cli-target-group-attach-fails-with-invalid-arn [mismatch]:
AssertionError: Command failed (rc=254):
cmd: ['aws', '--output', 'json', 'autoscaling', 'create-auto-scaling-group', '--auto-scaling-group-name', '<name>-asg', '--launch-template', 'LaunchTemplateId=lt-...,Version=$Latest', '--min-size', '1', '--max-size', '1', '--desired-capacity', '1', '--vpc-zone-identifier', 'subnet-...,subnet-...']
stderr: aws: [ERROR]: An error occurred (NotImplemented) when calling the CreateAutoScalingGroup operation: launching instances for a group that names only VPCZoneIdentifier needs each subnet's Availability Zone, which this emulator does not hold: pass AvailabilityZones as well
Expected: the create succeeds (200, empty CreateAutoScalingGroupResponse) with the group's AvailabilityZones derived from the two named subnets; error code NotImplemented / HTTP 501 is not in the operation's modeled error set (AlreadyExists 400, LimitExceeded 400, ResourceContention 500, ServiceLinkedRoleFailure 500). The subnet ids and group name in the command are environment-specific examples, not requirements.

Cause 2 — autoscaling__ec2__elbv2.cli-asg-attach-load-balancer-target-groups-command [error] and autoscaling__ec2__elbv2.cli-create-asg-with-target-group-and-verify-ec2-registration [error], both identical:
File "parsers.py", line 600, in _do_error_parse -> botocore.parsers.ResponseParserError: Unable to parse response (not well-formed (invalid token): line 1, column 0), invalid XML received. Further retries may succeed:
b'{"__type":"AmbiguousService","message":"2 emulated services all claim requests for \'elasticloadbalancing\': elasticloadbalancing, elasticloadbalancing; the request is rejected rather than served by an arbitrary one"}'
Expected: the addressed elbv2 operation's normal 200 XML response (or, for a genuine failure, an XML ErrorResponse carrying a modeled error code of that service).

## Steps to Reproduce
Boot the emulator from mainline and point the AWS CLI / botocore clients for 'autoscaling', 'ec2' and 'elbv2' at it in us-west-2, then:
1. Cause 1: create a launch template in EC2, then run `aws autoscaling create-auto-scaling-group --auto-scaling-group-name <n> --launch-template LaunchTemplateId=<lt>,Version=$Latest --min-size 1 --max-size 1 --desired-capacity 1 --vpc-zone-identifier <subnet-a>,<subnet-b>` with NO --availability-zones — observe error code NotImplemented and that no group is created; repeating the identical call WITH --availability-zones is the control.
2. Cause 2: with both load-balancing crates registered, call any elbv2 operation (e.g. `aws elbv2 create-target-group ...` or elbv2.describe_target_health(...)) — observe the JSON AmbiguousService body and botocore's ResponseParserError instead of an XML response.
Equivalently, re-replay the behavioral checks listed in trace_ids.
