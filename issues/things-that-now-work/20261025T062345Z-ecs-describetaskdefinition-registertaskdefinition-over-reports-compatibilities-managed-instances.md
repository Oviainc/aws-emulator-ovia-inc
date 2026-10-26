Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.ecsdeploy-firelens-task-definition-round-trips-with-router-defaults
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.ecsdeploy-firelens-task-definition-round-trips-with-router-defaults|run#18|AssertionError|assert sorted(got["compatibilities"]) == ["EC2", "FARGATE"], (
Issue-Commit: cfbb9a1e24eec7d8c98af7d6e9e62a0e8013f018

# ECS DescribeTaskDefinition/RegisterTaskDefinition over-reports compatibilities: MANAGED_INSTANCES is added to every awsvpc task definition

> _Note: this issue was identified against the code base at commit `cfbb9a1e24eec7d8c98af7d6e9e62a0e8013f018`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTaskDefinition

## Description
1 cause.

Cause 1 of 1: the derived `TaskDefinition$compatibilities` list contains an extra launch type (`MANAGED_INSTANCES`) for any task definition registered with `networkMode: awsvpc`. Failing check: ecs.ecsdeploy-firelens-task-definition-round-trips-with-router-defaults.

AWS: `TaskDefinition$compatibilities` is DERIVED by the service, not echoed from the request — Smithy: `com.amazonaws.ecs#TaskDefinition$compatibilities` targets `CompatibilityList`/`Compatibility` (values `EC2`, `FARGATE`, `EXTERNAL`, `MANAGED_INSTANCES`) and documents "Amazon ECS validates the task definition parameters with those supported by the launch type." The model states which values exist but not the derivation rule, so for the rule itself this is recorded behavior only: the check (audited against real AWS, account-leased, 2026-09-17) registers a Fargate-shaped revision — `networkMode: awsvpc`, task-level `cpu: "2048"` / `memory: "4096"`, `requiresCompatibilities: ["FARGATE"]`, `ephemeralStorage.sizeInGiB: 40`, two containers (a non-essential `fluentbit` FireLens router plus the service container) — and the recorded response reports `compatibilities` of exactly `EC2` and `FARGATE`. `MANAGED_INSTANCES` is NOT reported merely because the network mode is `awsvpc`; `requiresCompatibilities` (the launch types the registration asked to be validated against) stays `["FARGATE"]` in the same response. Note every other field-level assertion in this check passed (container order, images, router `firelensConfiguration`, injected router `user` "0", `hostPort` defaulting, option-less `awsfirelens` driver, task-level cpu/memory/ephemeralStorage, requiresAttributes), so the divergence is confined to this one derived member.

Emulator: the same describe returns three launch types — `['EC2', 'MANAGED_INSTANCES', 'FARGATE']` — because the derivation treats `awsvpc` alone as sufficient for ECS Managed Instances. The suite reports `AssertionError: compatibilities are COMPUTED from the definition; got ['EC2', 'MANAGED_INSTANCES', 'FARGATE']` (expected `['EC2', 'FARGATE']` after sorting). HTTP status is 200 in both cases; this is a response-content divergence, not a refusal.

Why: `compatibilities` is what a client (and the console, CDK, and deployment tooling) reads to decide which launch types a revision may be placed with. A client that offers or auto-selects a launch type from this list will offer ECS Managed Instances for a revision real ECS never validated for it, and the subsequent placement is refused by real AWS while succeeding here. The same derived list is also the gate used to validate a requested `launchType` on placement, so the emulator additionally ACCEPTS `launchType: MANAGED_INSTANCES` for revisions AWS rejects, which makes the over-report change behavior and not just the rendered payload.

Fix: the task-definition projection's derivation of `compatibilities` in the ECS service state — the per-launch-type predicate, specifically the `MANAGED_INSTANCES` arm, which must require more than `networkMode == awsvpc` (the recorded response shows an awsvpc + Fargate-sized revision reporting only `EC2` and `FARGATE`). Because the launch-type validation helper shares this function, tightening it there fixes both the reported list and the placement gate; no other member of the projection needs to change.

Traces: ecs.ecsdeploy-firelens-task-definition-round-trips-with-router-defaults. Code: `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/state.rs` — `COMPATIBILITY_ORDER` (line 535), `TaskDefinitionRecord::compatibilities()` (lines 1566-1580, the `"MANAGED_INSTANCES" => awsvpc` arm at line 1573), rendered into the projection at line 1651, and reused as the placement gate by `validate_launch_type` (lines 3940-3954). Reached from `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/register_task_definition.rs` line 503 (`record.view()`).

## Evidence
Check: ecs.ecsdeploy-firelens-task-definition-round-trips-with-router-defaults (verdict: mismatch).

Request (RegisterTaskDefinition): family=<unique>, cpu="2048", memory="4096", networkMode="awsvpc", requiresCompatibilities=["FARGATE"], executionRoleArn=<role>, ephemeralStorage={"sizeInGiB": 40}, containerDefinitions=[{name:"log-router", image:"registry.invalid/sim2real-ecsdeploy:fluent-bit-latest", cpu:256, memoryReservation:512, essential:false, firelensConfiguration:{type:"fluentbit"}, logConfiguration:{logDriver:"awslogs", ...}}, {name:"service", image:"registry.invalid/sim2real-ecsdeploy:latest", cpu:1792, memory:4096, essential:true, portMappings:[{containerPort:8443, protocol:"tcp"}], logConfiguration:{logDriver:"awsfirelens"}}]

Then DescribeTaskDefinition(taskDefinition=<arn>).

Expected (recorded real AWS): taskDefinition.compatibilities == ["EC2", "FARGATE"] (sorted), alongside requiresCompatibilities == ["FARGATE"]. HTTP 200.

Actual (emulator): taskDefinition.compatibilities == ['EC2', 'MANAGED_INSTANCES', 'FARGATE']. HTTP 200.

Suite failure text:
  File "check_ecs_ecsdeploy_firelens_task_definition_round_trips_with_router_defaults.py", line 64, in run
    assert sorted(got["compatibilities"]) == ["EC2", "FARGATE"]
  AssertionError: compatibilities are COMPUTED from the definition; got ['EC2', 'MANAGED_INSTANCES', 'FARGATE']

All preceding assertions in the same check (container order/images, router firelensConfiguration/essential/memoryReservation/cpu, injected router user "0" and no user on the service container, awsvpc portMappings gaining hostPort==containerPort, option-less awsfirelens logConfiguration, task-level cpu/memory/networkMode/requiresCompatibilities/ephemeralStorage/status/revision) passed, isolating the defect to the derived compatibilities list.

## Steps to Reproduce
1. Boot the emulator and point an ecs client at it.
2. RegisterTaskDefinition with networkMode="awsvpc", task-level cpu="2048" and memory="4096", requiresCompatibilities=["FARGATE"], an executionRoleArn, ephemeralStorage={"sizeInGiB":40}, and two container definitions (a non-essential fluentbit FireLens router plus an essential service container publishing containerPort 8443).
3. Call DescribeTaskDefinition on the returned taskDefinitionArn and read taskDefinition.compatibilities.
4. Observe ['EC2','MANAGED_INSTANCES','FARGATE']; real AWS reports only EC2 and FARGATE for this definition.
5. Secondary effect: RunTask/CreateService with launchType="MANAGED_INSTANCES" against the same revision is accepted by the emulator because the launch-type validation reads the same derived list.
