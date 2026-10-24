Issue-Severity: MEDIUM

# ECS CreateService accepts an instance-type ELBv2 target group for an awsvpc task definition

> _Filed by the fixer of
> `20261023T025808Z-ecr-ecs-elbv2-ecs-service-creation-against-a-mismatched-elbv2-target.md`
> (the shared Elastic Load Balancing endpoint now serves the ELBv2 API at version
> 2015-12-01, so this call is reachable for the first time and the gap below became
> observable). The merge-gate reviewer of that fix asked for it to be recorded._

## Operation
CreateService (`com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113`)

## Description
`CreateService` stores the request's `loadBalancers` verbatim
(`services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_service.rs`,
the `load_balancers: input.load_balancers.clone().unwrap_or_default()` field of the
`ServiceRecord` it inserts) and checks nothing about the target group each entry names.

AWS refuses the create when the target group's `TargetType` cannot carry the task
definition's `networkMode`: a service whose task definition declares `awsvpc` registers
its tasks by IP, so its target groups must be `ip` (or `alb`), and a target group of
`TargetType` `instance` is rejected with `InvalidParameterException`. The emulator
answers `200` and persists the service, so a caller that asserts on the refusal sees the
call succeed.

Observed through the shared `elasticloadbalancing` endpoint at API version 2015-12-01:

1. ELBv2 `CreateTargetGroup` with `TargetType=instance`.
2. ECS `RegisterTaskDefinition` with `networkMode=awsvpc`.
3. ECS `CreateService` naming that task definition and that target group in
   `loadBalancers[0].targetGroupArn` → the emulator answers `200` with the created
   service; AWS answers `400 InvalidParameterException`.

## Expected
`CreateService` validates each `loadBalancers` entry against the resource it names — the
target group's `TargetType` against the task definition's `networkMode`, as AWS
documents for `awsvpc` — and refuses a mismatch with the modelled
`InvalidParameterException` (HTTP 400). The target group's type lives in the ELBv2
service's state, so the check needs a cross-service read (as other ECS validations reach
other services), not a value copied from the request.

## Notes
The ELBv2 side of this scenario works: `CreateLoadBalancer`, `CreateTargetGroup` and
`CreateListener` at `Version=2015-12-01` are served through the shared endpoint and read
back from state (see
`services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/tests/all/shared_endpoint_api_versions.rs`).
Only the ECS-side validation of the pairing is missing.

## Resolution
Fixed in `src/state.rs` (`required_target_type`, `requested_target_group_arns`,
`target_group_target_types`, `load_balancer_refusal`, `target_group_type_refusal`),
applied by `CreateService` and `UpdateService`, with
`services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/tests/all/load_balancer_target_groups.rs`
driving the pairing class across both services over the wire.

One correction to the Description above, recorded so a later reader does not take the
issue text for the rule: this report says an `awsvpc` service's target group "must be `ip`
(or `alb`)". The fix accepts `ip` ONLY, following the model — `LoadBalancer$targetGroupArn`
says "you must choose `ip` as the target type" because an `awsvpc` task is registered by
the IP of its own elastic network interface, while the targets of an `alb` target group
are load balancers, which no ECS task registration produces. `alb` is refused for every
network mode, and the test table pins that.

