Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.ecs-service-deregisters-tasks-on-scale-down
Issue-Key: autoscaling__ecs__elbv2.ecs-service-deregisters-tasks-on-scale-down|_target_health#5|Timeout|return elbv2.describe_target_health(TargetGroupArn=tg_arn)["TargetHealthDescriptions"]
Issue-Commit: 6596661b7bed7b2c68eacfc83c0e7a703969540b

# autoscaling__ecs__elbv2: scaling a load-balanced Fargate service to desiredCount 0 never drains and deregisters its ip target (no 'draining' state, target never removed)

> _Note: this issue was identified against the code base at commit `6596661b7bed7b2c68eacfc83c0e7a703969540b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateService

## Description
One net-new root cause under the suite heading `autoscaling__ecs__elbv2`, grounded in a failing check that no open issue carries the current replay key for.

Cause 1 of 1: the scale-in half of the ECS<->ELBv2 target lifecycle never happens. A Fargate/awsvpc service created with a `loadBalancers` entry, then updated to `desiredCount=0`, never shows its registered ip target passing through `draining` and never has it removed from the target group, so the whole check exhausts its budget with no terminal observation. Failing check id: autoscaling__ecs__elbv2.ecs-service-deregisters-tasks-on-scale-down.
  AWS: recorded behavior only — the ECS/ELBv2 reconciliation is not described by the Smithy model (which only gives the shapes: `ecs#UpdateService.desiredCount`, `ecs#Service.runningCount`, `elasticloadbalancingv2#TargetHealthDescription.TargetHealth.State` with the `draining` state, and the target-group attribute `deregistration_delay.timeout_seconds`). In the recording: the service's task reaches RUNNING and its ENI private IP is registered as an `ip` target; after `UpdateService(desiredCount=0)` ECS calls DeregisterTargets, so `DescribeTargetHealth` reports the target with `TargetHealth.State == "draining"`, it stays draining for about the configured `deregistration_delay.timeout_seconds` (10s here), and only then disappears from `TargetHealthDescriptions`. The requirement is the ordered transition registered -> draining -> absent, held for the configured delay; the specific delay value and the target IP are per-run inputs/state, not part of the contract.
  Emulator: nothing in that sequence is ever observed — the check ends with `Error: Check timed out after 180.0s` while polling `DescribeTargetHealth`. The emulator produced no transition at all (neither an early removal nor a stuck `draining`), so the ordered contract cannot be evaluated: either the service's task/target never becomes registered, or the scale-in never propagates into DeregisterTargets.
  Why: a client that scales a load-balanced service in and waits for connection draining to finish (the standard safe-scale-in and blue/green cutover gate) waits forever; it can neither confirm that in-flight requests were drained for the configured delay nor that the target was finally removed, and any automation keyed on `draining` never advances.
  Fix: the ECS service/task reconciliation and its propagation into ELBv2 target registration — specifically the scale-in path that must call DeregisterTargets for the removed task and the ELBv2 target state machine that must hold the target `draining` for `deregistration_delay.timeout_seconds` before dropping it (ECS crate's service/task state machine plus the ELBv2 target-health state).
  Related open issue that does NOT carry this check's current replay key: 20261104T042502Z-autoscaling-ecs-elbv2-6-net-new-root-causes.md (its cause 1 covers the forward direction — tasks reaching RUNNING and registering — under a different replay key, and the suite did not mark this failure as owned by it).
  Traces: autoscaling__ecs__elbv2.ecs-service-deregisters-tasks-on-scale-down.

## Evidence
Cause 1 — expected: after `update_service(desiredCount=0)` on a Fargate service with `loadBalancers=[{targetGroupArn, containerName, containerPort}]`, `describe_target_health(TargetGroupArn=<tg>)` first reports the task's ip target with `TargetHealth.State == "draining"`, and the target is gone only ~`deregistration_delay.timeout_seconds` later. Actual: `Error: Check timed out after 180.0s` — no registered target, no `draining` state and no removal is ever observed within the check's budget (the check itself allows 180s for registration and a further 300s for drain-then-removal).

## Steps to Reproduce
1. Create a VPC with two public subnets and a security group; create an ALB, an `ip`-type target group on port 80 with `deregistration_delay.timeout_seconds=10` (via `modify_target_group_attributes`) and an HTTP:80 listener forwarding to it.
2. Register a FARGATE/awsvpc task definition (httpd, containerPort 80) and `create_service(..., launchType="FARGATE", desiredCount=1, loadBalancers=[{targetGroupArn:<tg>, containerName:"web", containerPort:80}])`.
3. Poll `elbv2 describe_target_health(TargetGroupArn=<tg>)` until one target is registered.
4. `ecs update_service(cluster=<c>, service=<s>, desiredCount=0)` and keep polling `describe_target_health`: expect the target to appear as `draining`, then vanish after the configured delay. Neither is observed.
Replay check id: autoscaling__ecs__elbv2.ecs-service-deregisters-tasks-on-scale-down.
