Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-container-secret-base64-payload-round-trip
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-container-secret-base64-payload-round-trip ecs-err-run-task-fargate-on-ec2-task-definition ecs-task-definition-lifecycle ecs-task-definition-revision-lifecycle ecs-fargate-awsvpc-task-definition-rendered-defaults ecs-run-task-and-task-protection ecs-service-lifecycle-on-cluster ecs-task-set-external-deployment-workflow ecs-taskdef-tagging-lifecycle ecs-taskset-basic-lifecycle ecs-taskset-lifecycle-ec2 ecs-taskset-errors
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|RegisterTaskDefinition|2xx->2xx|$.taskDefinition.compatibilities[*]
Issue-Key: ecs|RegisterTaskDefinition|2xx->2xx|$.taskDefinition.registeredBy
Issue-Key: ecs|RegisterTaskDefinition|2xx->2xx|$.taskDefinition.requiresAttributes
Issue-Key: ecs|DescribeTaskDefinition|2xx->2xx|$.tags
Issue-Key: ecs|DescribeTaskDefinition|2xx->2xx|$.taskDefinition.compatibilities[*]
Issue-Key: ecs|DescribeTaskDefinition|2xx->2xx|$.taskDefinition.registeredBy
Issue-Key: ecs|DescribeTaskDefinition|2xx->2xx|$.taskDefinition.requiresAttributes
Issue-Key: ecs|RegisterTaskDefinition|2xx->2xx|$.taskDefinition.containerDefinitions[*].portMappings[*].hostPort
Issue-Key: ecs|RegisterTaskDefinition|2xx->2xx|$.taskDefinition.containerDefinitions[*].portMappings[*].protocol
Issue-Key: ecs|DescribeTaskDefinition|2xx->2xx|$.taskDefinition.containerDefinitions[*].portMappings[*].hostPort
Issue-Key: ecs|DescribeTaskDefinition|2xx->2xx|$.taskDefinition.containerDefinitions[*].portMappings[*].protocol
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# ECS task-definition projection drops registeredBy, requiresAttributes and port-mapping defaults

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTaskDefinition

## Description
The TaskDefinition the emulator renders is incomplete and mis-ordered: `registeredBy` is never set, `requiresAttributes` is never derived from the container definitions, container port mappings are not defaulted, the `compatibilities` list is emitted in a different order than AWS (and omits MANAGED_INSTANCES for Fargate-capable definitions), and DescribeTaskDefinition omits the top-level empty `tags` list. The same projection backs RegisterTaskDefinition and DescribeTaskDefinition, so every read of a registered definition is affected.

THIS BUNDLE CONTAINS 7 CAUSES.

Cause 1 of 7 — RegisterTaskDefinition: compatibilities mis-ordered, registeredBy and requiresAttributes absent.
AWS: Smithy: TaskDefinition.compatibilities : CompatibilityList, TaskDefinition.registeredBy : String ("The principal that registered the task definition"), TaskDefinition.requiresAttributes : RequiresAttributes ("The container instance attributes required by your task"). Recording: compatibilities=[EXTERNAL, EC2], registeredBy='arn:aws:iam::<account>:user/<caller>', requiresAttributes=[{name: ecs.capability.secrets.asm.environment-variables}] — derived from the definition's use of Secrets Manager secrets.
Emulator: compatibilities=[EC2, EXTERNAL], registeredBy absent, requiresAttributes absent. Status 200 both sides.
Why: a client that checks requiresAttributes before placing a task on an instance has nothing to check, and one that reads compatibilities positionally reads the wrong launch type; registeredBy-based auditing is impossible.
Fix: the task-definition projection and capability derivation in services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/register_task_definition.rs.
Traces: ecs-container-secret-base64-payload-round-trip#task#3.

Cause 2 of 7 — DescribeTaskDefinition: same three defects plus the empty tags list.
AWS: Smithy: DescribeTaskDefinition -> output {taskDefinition, tags : Tags}. Recording: tags=[] is present.
Emulator: tags absent, plus the Cause-1 deltas.
Why: clients indexing the tags member of the describe response break on its absence.
Fix: services/.../src/ops/describe_task_definition.rs output projection.
Traces: ecs-container-secret-base64-payload-round-trip#task#4.

Cause 3 of 7 — DescribeTaskDefinition on an EC2 definition: tags, compatibilities order and registeredBy.
AWS: recording: tags=[], compatibilities=[EXTERNAL, EC2], registeredBy set.
Emulator: tags absent, compatibilities=[EC2, EXTERNAL], registeredBy absent.
Fix: as Cause 2.
Traces: ecs-err-run-task-fargate-on-ec2-task-definition#task#4 (+2 members, e.g. ecs-task-definition-lifecycle#task#3).

Cause 4 of 7 — Fargate/awsvpc registration: MANAGED_INSTANCES missing from compatibilities, port-mapping defaults not rendered, registeredBy and requiresAttributes absent.
AWS: Smithy: TaskDefinition.requiresCompatibilities documents the valid values as MANAGED_INSTANCES, EC2, FARGATE and EXTERNAL; PortMapping defaults hostPort and protocol. Recording: compatibilities[1]='MANAGED_INSTANCES', containerDefinitions[0].portMappings[0].hostPort=80, protocol='tcp', requiresAttributes=[{name: com.amazonaws.ecs.capability.docker-remote-api.1.18}, {name: ecs.capability.task-eni}].
Emulator: compatibilities[1]='FARGATE', hostPort and protocol absent, registeredBy and requiresAttributes absent.
Why: a client reading back the rendered definition cannot see the host port or protocol AWS filled in, so awsvpc port wiring differs from the real service.
Fix: the port-mapping defaulting and compatibility computation in services/.../src/ops/register_task_definition.rs.
Traces: ecs-fargate-awsvpc-task-definition-rendered-defaults#task#2.

Cause 5 of 7 — the same Fargate definition read back by DescribeTaskDefinition.
AWS: as Cause 4 plus tags=[].
Emulator: as Cause 4 plus tags absent.
Fix: as Cause 2/4.
Traces: ecs-fargate-awsvpc-task-definition-rendered-defaults#task#3.

Cause 6 of 7 — RegisterTaskDefinition: compatibilities order and registeredBy on a plain EC2 definition.
AWS: recording: compatibilities=[EXTERNAL, EC2], registeredBy='arn:aws:iam::<account>:user/<caller>'.
Emulator: compatibilities=[EC2, EXTERNAL], registeredBy absent.
Fix: as Cause 1.
Traces: ecs-run-task-and-task-protection#setup#3 (+8 members, e.g. ecs-taskdef-tagging-lifecycle#setup#2).

Cause 7 of 7 — bridge-mode port mapping: hostPort default 0 not rendered.
AWS: recording: containerDefinitions[0].portMappings[0].hostPort=0 for a bridge-mode mapping with no host port supplied, compatibilities=[EXTERNAL, EC2], registeredBy set.
Emulator: hostPort absent, compatibilities=[EC2, EXTERNAL], registeredBy absent.
Why: a client that reads hostPort to learn the assigned dynamic port finds the member missing.
Fix: as Cause 4.
Traces: ecs-taskset-errors#setup#3.

## Evidence
Cause 1 (ecs-container-secret-base64-payload-round-trip#task#3, RegisterTaskDefinition, 200/200):
  $.taskDefinition.compatibilities[0]: value_mismatch (expected 'EXTERNAL', got 'EC2')
  $.taskDefinition.compatibilities[1]: value_mismatch (expected 'EC2', got 'EXTERNAL')
  $.taskDefinition.registeredBy: missing (expected 'arn:aws:iam::000000000000:user/emulator', got None)
  $.taskDefinition.requiresAttributes: missing (expected [{'name': 'ecs.capability.secrets.asm.environment-variables'}], got None)
Cause 4 (ecs-fargate-awsvpc-task-definition-rendered-defaults#task#2):
  $.taskDefinition.compatibilities[1]: missing (expected 'MANAGED_INSTANCES', got 'FARGATE')
  $.taskDefinition.containerDefinitions[0].portMappings[0].hostPort: missing (expected 80, got None)
  $.taskDefinition.containerDefinitions[0].portMappings[0].protocol: missing (expected 'tcp', got None)
  $.taskDefinition.requiresAttributes: missing (expected [{'name': 'com.amazonaws.ecs.capability.docker-remote-api.1.18'}, {'name': 'ecs.capability.task-eni'}], got None)
Cause 2/3/5 add: $.tags: missing (expected [], got None) on DescribeTaskDefinition.
Cause 7 (ecs-taskset-errors#setup#3): $.taskDefinition.containerDefinitions[0].portMappings[0].hostPort: missing (expected 0, got None).

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus (replay_wire_corpus).
2. Fixture ecs-container-secret-base64-payload-round-trip steps task#3/task#4 and ecs-fargate-awsvpc-task-definition-rendered-defaults steps task#2/task#3.
3. By hand: RegisterTaskDefinition with a container that references a Secrets Manager secret, then DescribeTaskDefinition on the returned revision — compatibilities order differs, registeredBy/requiresAttributes are absent, port mappings are not defaulted, and the describe response has no tags member.
