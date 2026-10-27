Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.ecsdeploy-firelens-task-definition-round-trips-with-router-defaults
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.ecsdeploy-firelens-task-definition-round-trips-with-router-defaults|run#20|AssertionError|assert {"ecs.capability.firelens.fluentbit", "com.amazonaws.ecs.capability.logging-driver.awsfirelens",
Issue-Commit: da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a

# ECS requiresAttributes derivation omits the FireLens and execution-role-awslogs capabilities

> _Note: this issue was identified against the code base at commit `da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTaskDefinition

## Description
THIS BUNDLE CONTAINS 1 CAUSE.

Cause 1 of 1 — the requiresAttributes list the emulator derives for a task definition is incomplete: the FireLens log-router capability and the awslogs-with-execution-role capability are never derived, even though the sibling logging-driver and task-ENI capabilities now are.

AWS: Smithy: TaskDefinition.requiresAttributes : RequiresAttributes, documented as "The container instance attributes required by your task" — i.e. the member is COMPUTED by ECS from the submitted containerDefinitions, not echoed from the request (RegisterTaskDefinition's input has no requiresAttributes member at all). Each element is an Attribute with a required `name` (1-128 chars). The derivation is per-feature: a container carrying firelensConfiguration (Smithy: FirelensConfiguration.type is a required FirelensConfigurationType whose valid values are `fluentd` or `fluentbit`) requires the matching FireLens router capability `ecs.capability.firelens.<type>`; a container whose logConfiguration.logDriver is `awslogs` on a definition that carries an executionRoleArn requires `ecs.capability.execution-role-awslogs`; the log drivers themselves require `com.amazonaws.ecs.capability.logging-driver.<driver>`; awsvpc networkMode requires `ecs.capability.task-eni`. The names are fixed text parameterised only by the feature they are derived from (the firelensConfiguration type, the log-driver name); nothing in them comes from the account, the family or the revision. Recorded behavior for this shape of definition (a two-container Fargate/awsvpc definition: a non-essential `fluentbit` FireLens router whose own logConfiguration is awslogs against a log group, plus a service container using the option-less `awsfirelens` driver, registered with an executionRoleArn) is a 200 whose requiresAttributes is a superset of {ecs.capability.firelens.fluentbit, com.amazonaws.ecs.capability.logging-driver.awsfirelens, ecs.capability.execution-role-awslogs, ecs.capability.task-eni}, and which must NOT contain ecs.capability.execution-role-ecr-pull because the images are not ECR URIs.

Emulator: the call succeeds (200) and the definition round-trips correctly in every other graded respect — container order, images, the router's firelensConfiguration/essential/memoryReservation/cpu, the injected user "0" on the router only, the awsvpc hostPort==containerPort defaulting, the option-less awsfirelens logConfiguration, task-level cpu/memory/ephemeralStorage and the computed compatibilities all match. Only requiresAttributes is short. DescribeTaskDefinition on the returned revision returns exactly ['com.amazonaws.ecs.capability.docker-remote-api.1.18', 'com.amazonaws.ecs.capability.logging-driver.awsfirelens', 'com.amazonaws.ecs.capability.logging-driver.awslogs', 'ecs.capability.task-eni'] — so `ecs.capability.firelens.fluentbit` (derived from the router's firelensConfiguration.type) and `ecs.capability.execution-role-awslogs` (derived from the awslogs driver combined with the executionRoleArn) are both absent. The ECR-pull capability is correctly absent. No error code and no status divergence: this is a content defect in a 2xx body.

Why: requiresAttributes is the contract a client uses to decide whether a container instance can run the task. A placement decision made against this list sees a FireLens task definition that claims to need no FireLens router capability and no execution-role-awslogs capability, so it will schedule the task onto instances whose agent cannot honour the log router, and tooling that pre-flights a definition against ListAttributes/DescribeContainerInstances attributes reports a task as placeable where real ECS would refuse or fail at launch. It also makes the two capability-bearing features invisible to anything that audits a definition's feature surface by reading the derived list.

Fix: the capability-derivation step of the task-definition projection in the ECS crate's register_task_definition handler (the same projection backs describe_task_definition, so the omission is visible on both). The derivation already walks containers for log drivers and the definition for awsvpc; it needs the two missing rules — firelensConfiguration.type -> ecs.capability.firelens.<type>, and awslogs driver + executionRoleArn present -> ecs.capability.execution-role-awslogs. Note the resolved issue 20261022T152302Z-ecs-task-definition-projection-drops-registeredby-requiresattributes-and.md introduced requiresAttributes derivation at all; that fix landed the docker-remote-api, logging-driver and task-eni rules but not these two, so this is the remaining gap in the same component rather than a regression of a refusal.

Traces: ecs.ecsdeploy-firelens-task-definition-round-trips-with-router-defaults.

## Evidence
Check ecs.ecsdeploy-firelens-task-definition-round-trips-with-router-defaults — verdict: mismatch.

Assertion that failed (check_ecs_ecsdeploy_firelens_task_definition_round_trips_with_router_defaults.py line 67):
  assert {"ecs.capability.firelens.fluentbit", "com.amazonaws.ecs.capability.logging-driver.awsfirelens",
          "ecs.capability.execution-role-awslogs", "ecs.capability.task-eni"} <= attrs

  AssertionError: requiresAttributes must name the FireLens, awslogs and task-ENI capabilities; got
  ['com.amazonaws.ecs.capability.docker-remote-api.1.18',
   'com.amazonaws.ecs.capability.logging-driver.awsfirelens',
   'com.amazonaws.ecs.capability.logging-driver.awslogs',
   'ecs.capability.task-eni']

Expected (AWS): requiresAttributes contains at least
  ecs.capability.firelens.fluentbit
  com.amazonaws.ecs.capability.logging-driver.awsfirelens
  ecs.capability.execution-role-awslogs
  ecs.capability.task-eni
and does NOT contain ecs.capability.execution-role-ecr-pull.

Actual (emulator): the two names ecs.capability.firelens.fluentbit and ecs.capability.execution-role-awslogs are missing from the list; the other two required names and the ECR-pull exclusion are correct.

Both RegisterTaskDefinition and the following DescribeTaskDefinition return HTTP 200 with no error code; every other field-level assertion in the check (container order, images, router firelensConfiguration/essential/memoryReservation/cpu, injected router user "0" and no user on the service container, awsvpc hostPort==containerPort, option-less awsfirelens logConfiguration, cpu="2048"/memory="4096"/networkMode=awsvpc/requiresCompatibilities=[FARGATE]/ephemeralStorage.sizeInGiB=40/status=ACTIVE/revision=1, compatibilities=[EC2, FARGATE]) passed before this one, so the divergence is isolated to the derived capability list.

Smithy citations (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/smithy-model/com-amazonaws-ecs-amazonec2containerservicev20141113.json):
  TaskDefinition.requiresAttributes : RequiresAttributes — "The container instance attributes required by your task" (computed; absent from RegisterTaskDefinition input)
  Attribute.name : String, required
  FirelensConfiguration.type : FirelensConfigurationType, required — valid values fluentd | fluentbit

## Steps to Reproduce
1. Boot the emulator and replay the behavioral check ecs.ecsdeploy-firelens-task-definition-round-trips-with-router-defaults.
2. By hand: create an IAM role trusted by ecs-tasks.amazonaws.com with AmazonECSTaskExecutionRolePolicy attached, and a CloudWatch Logs log group.
3. Call RegisterTaskDefinition with family=<any>, cpu="2048", memory="4096", networkMode="awsvpc", requiresCompatibilities=["FARGATE"], executionRoleArn=<the role ARN>, ephemeralStorage={sizeInGiB: 40} and two containerDefinitions in this order:
   a. name="log-router", image="<registry>/<repo>:fluent-bit-latest", cpu=256, memoryReservation=512, essential=false, firelensConfiguration={type: "fluentbit"}, logConfiguration={logDriver: "awslogs", options: {awslogs-group: <log group>, awslogs-region: <region>, awslogs-stream-prefix: "log-router"}}
   b. name="service", image="<registry>/<repo>:latest", cpu=1792, memory=4096, essential=true, portMappings=[{containerPort: 8443, protocol: "tcp"}], logConfiguration={logDriver: "awsfirelens"}
   Use a non-ECR registry host for both images.
4. Call DescribeTaskDefinition on the returned taskDefinitionArn and read taskDefinition.requiresAttributes.
5. Observe the list omits ecs.capability.firelens.fluentbit and ecs.capability.execution-role-awslogs. Real ECS includes both (alongside the logging-driver and task-eni entries the emulator does emit), and still omits ecs.capability.execution-role-ecr-pull for these non-ECR images.
