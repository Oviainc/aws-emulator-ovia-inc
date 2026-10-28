Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.deploy-ecs-service-with-alb-target-group-cfn
Issue-Key: elastic-load-balancing-v2|DescribeTargetHealth|InvalidAction
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# ELBv2 DescribeTargetHealth is not dispatchable at version 2015-12-01, so a CFN-deployed ECS service's ALB target health can never be read

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Resolution (what was fixed here, and which part this record never owned)

**Cause 1 — FIXED.** The `(Action, Version)` pair an awsQuery envelope names now resolves to
the model that declares the version, so `Action=DescribeTargetHealth&Version=2015-12-01` on
the shared `elasticloadbalancing` endpoint reaches this operation and is answered `200` with
one `TargetHealthDescription` per target — derived from the registrations the store holds —
or with the operation's own modelled `TargetGroupNotFound` (400) for a target group nothing
holds. `InvalidAction` is left for a pair no mounted model declares. The front door that
resolves the pair is
`services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/service/api_version.rs`
(`serving_model` + `V2FrontDoor`, reached from that crate's `src/service.rs`); it landed from
the sibling records
`20261023T025808Z-elbv2-awsquery-action-table-at-api-version-2015.md` and
`20261022T220446Z-elbv2-elastic-load-balancing-v2-is-unreachable-every-request.md` after this
record was filed against commit `1b15cfcf`. What this record adds is the coverage its own key
had nowhere: the WHOLE declared action set, read from the model itself, driven at
`2015-12-01` through both front doors
(`services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/tests/all/declared_action_set.rs`),
the recorded action end to end over a table of request shapes in the same file, and the
production-router rows in
`core/router/tests/all/elb_classic_and_v2_signing_name_routing.rs` that register a target and
read its health back on the shared signing name.

**Not this record's key.** The title's deployed-stack half needs its own record:
CloudFormation leaves `AWS::ElasticLoadBalancingV2::*` declared-only — `provisioner_for` in
`services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs` has no
arm for those types, and `src/state/template.rs` synthesizes their physical ids and ARNs — so
a target group a stack deploys is not one Elastic Load Balancing holds, and a health read
addressed at a synthesized ARN is the modelled `TargetGroupNotFound`. That is CloudFormation's
resource-provisioning fidelity, the same split the sibling record above made for the
CloudFormation causes it carried, and not this model's action table.

## Operation
DescribeTargetHealth

## Description
One root cause, grounded in 1 failing behavioral check whose id's leading token is the composite 'autoscaling__ecs__elbv2'. The already-open issue 20261023T004835Z-acm-elbv2-elbv2-createloadbalancer-is-not-dispatchable-could-not-find.md names only CreateLoadBalancer under the 'acm__elbv2' token and does not carry this replay key.

Cause 1 of 1: the ELBv2 awsQuery action table does not resolve DescribeTargetHealth at API version 2015-12-01.
AWS: Smithy com.amazonaws.elasticloadbalancingv2#DescribeTargetHealth (service version 2015-12-01, protocol awsQuery) takes TargetGroupArn (required), Targets, Include and returns {TargetHealthDescriptions}; its only modeled errors are TargetGroupNotFound (awsQuery code TargetGroupNotFound, HTTP 400), InvalidTarget (400) and HealthUnavailable (500). A well-formed Action=DescribeTargetHealth&Version=2015-12-01 POST is served 200 or refused with one of those three codes. 'InvalidAction' is reserved for an action the service does not have.
Emulator: the call is refused with Error.Code 'InvalidAction' and message 'Could not find operation DescribeTargetHealth for version 2015-12-01'.
Why: target health is the only way a client can tell whether an ECS service's tasks were registered behind an ALB and became healthy; with the read unreachable, every deploy-then-verify workflow is blind and the failure is misreported to the SDK as a nonexistent action.
Fix: the awsQuery operation dispatch/version table of services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10 (register DescribeTargetHealth and the rest of the 2015-12-01 action set).
Traces: autoscaling__ecs__elbv2.deploy-ecs-service-with-alb-target-group-cfn.

## Evidence
autoscaling__ecs__elbv2.deploy-ecs-service-with-alb-target-group-cfn — botocore.exceptions.ClientError: An error occurred (InvalidAction) when calling the DescribeTargetHealth operation: Could not find operation DescribeTargetHealth for version 2015-12-01.
Expected per Smithy: HTTP 200 with TargetHealthDescriptions, or HTTP 400 TargetGroupNotFound / InvalidTarget, or HTTP 500 HealthUnavailable.

## Steps to Reproduce
1. Boot the emulator.
2. boto3.client('elbv2').describe_target_health(TargetGroupArn='arn:aws:elasticloadbalancing:us-west-2:000000000000:targetgroup/sim2real-tg/0123456789abcdef').
3. Observe Error.Code 'InvalidAction' with message 'Could not find operation DescribeTargetHealth for version 2015-12-01' instead of 200 + TargetHealthDescriptions (or 400 TargetGroupNotFound).
4. Re-replay autoscaling__ecs__elbv2.deploy-ecs-service-with-alb-target-group-cfn.
