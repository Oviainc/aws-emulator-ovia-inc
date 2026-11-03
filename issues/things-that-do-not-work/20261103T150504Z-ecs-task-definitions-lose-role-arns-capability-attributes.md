Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-ecracc-fargate-ecr-image-execution-role-vs-task-role
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-ecracc-fargate-ecr-image-execution-role-vs-task-role ecs-ecracc-zero-desired-service-launch-type-for-an-ecr-image-without-execution-role ecs-fargate-task-definition-round-trips-awslogs-and-omits-entry-point
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|RegisterTaskDefinition|2xx->2xx|$.taskDefinition.requiresAttributes[name="com.amazonaws.ecs.capability.ecr-auth"]
Issue-Key: ecs|RegisterTaskDefinition|2xx->2xx|$.taskDefinition.requiresAttributes[name="com.amazonaws.ecs.capability.task-iam-role"]
Issue-Key: ecs|RegisterTaskDefinition|2xx->2xx|$.taskDefinition.requiresAttributes[name="ecs.capability.execution-role-ecr-pull"]
Issue-Key: ecs|RegisterTaskDefinition|2xx->2xx|$.taskDefinition.revision
Issue-Key: ecs|RegisterTaskDefinition|2xx->2xx|$.taskDefinition.taskDefinitionArn
Issue-Key: ecs|DescribeTaskDefinition|2xx->2xx|$.taskDefinition.executionRoleArn
Issue-Key: ecs|DescribeTaskDefinition|2xx->2xx|$.taskDefinition.requiresAttributes[name="com.amazonaws.ecs.capability.ecr-auth"]
Issue-Key: ecs|DescribeTaskDefinition|2xx->2xx|$.taskDefinition.requiresAttributes[name="com.amazonaws.ecs.capability.task-iam-role"]
Issue-Key: ecs|DescribeTaskDefinition|2xx->2xx|$.taskDefinition.requiresAttributes[name="ecs.capability.execution-role-ecr-pull"]
Issue-Key: ecs|DescribeTaskDefinition|2xx->2xx|$.taskDefinition.taskRoleArn
Issue-Key: ecs|DescribeTaskDefinition|2xx->2xx|$.taskDefinition.requiresAttributes
Issue-Key: ecs|DescribeTaskDefinition|2xx->2xx|$.taskDefinition.requiresAttributes[*]
Issue-Related: 20261029T220901Z-ecs-task-definition-and-list-operations-drop-previousstatus.md
Issue-Commit: 34977689197ee7e7ec3a117f2c4937da4e6f0436

# ECS task definitions lose role ARNs, capability attributes and compatibilities, and renumber revisions

> _Note: this issue was identified against the code base at commit `34977689197ee7e7ec3a117f2c4937da4e6f0436`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTaskDefinition

## Description
Four net-new causes, one fault: the TaskDefinition the emulator stores and returns is not the one that was registered — derived capability attributes and compatibilities are not computed, declared role ARNs are dropped, and the revision counter counts refused registrations. Open issue 20261029T220901Z-ecs-task-definition-and-list-operations-drop-previousstatus mentions MANAGED_INSTANCES compatibility for other operations and carries none of these replay keys.

Cause 1 of 4: RegisterTaskDefinition does not derive requiresAttributes, omits a compatibility, and over-counts the revision. AWS: for a Fargate definition with an ECR image, an execution role and a task role, taskDefinition.requiresAttributes includes 'com.amazonaws.ecs.capability.ecr-auth', 'com.amazonaws.ecs.capability.task-iam-role' and 'ecs.capability.execution-role-ecr-pull'; compatibilities lists MANAGED_INSTANCES alongside FARGATE (Smithy: Compatibility enum = [EC2, FARGATE, EXTERNAL, MANAGED_INSTANCES]); and the revision is 1 because the three REFUSED registrations before it consumed no revision number — so taskDefinitionArn ends ':1'. Emulator: requiresAttributes carries none of the three, compatibilities[1] is FARGATE where MANAGED_INSTANCES is expected, and the revision is 4 (ARN ends ':4'). Why: requiresAttributes is how a client decides whether a container instance can run the definition; and a revision counter that advances on refusals means a client that computes the next revision, or pins ':1', addresses the wrong definition. Fix: RegisterTaskDefinition's handler — derive requiresAttributes/compatibilities from the definition, and allocate a revision only on a successful registration. Traces: ecs-ecracc-fargate-ecr-image-execution-role-vs-task-role#task#6.

Cause 2 of 4: DescribeTaskDefinition drops executionRoleArn and taskRoleArn. AWS: both are echoed exactly as registered (Smithy: RegisterTaskDefinitionRequest.executionRoleArn / taskRoleArn -> TaskDefinition), together with the same three requiresAttributes entries and MANAGED_INSTANCES in compatibilities. Emulator: executionRoleArn and taskRoleArn are absent. Why: a client auditing which role a task assumes, or re-registering a definition from a describe, loses both roles — the re-registered definition then cannot pull or cannot assume its task role. Fix: the task-definition state (persist the role ARNs) and DescribeTaskDefinition's serializer. Traces: ecs-ecracc-fargate-ecr-image-execution-role-vs-task-role#task#7.

Cause 3 of 4: DescribeTaskDefinition returns no requiresAttributes at all for an ECR-image definition. AWS: requiresAttributes = [{name: 'com.amazonaws.ecs.capability.ecr-auth'}]. Emulator: the member is absent. Why: as above — placement capability is unreadable, and 'absent' is not the same as 'empty'. Fix: the derivation in RegisterTaskDefinition plus DescribeTaskDefinition's serializer. Traces: ecs-ecracc-zero-desired-service-launch-type-for-an-ecr-image-without-execution-role#task#8.

Cause 4 of 4: compatibilities omits MANAGED_INSTANCES and requiresAttributes is ordered/derived differently for an awslogs Fargate definition. AWS: compatibilities[2] = 'MANAGED_INSTANCES' and requiresAttributes[2] = {name: 'com.amazonaws.ecs.capability.docker-remote-api.1.19'}. Emulator: compatibilities has no third entry and requiresAttributes[2] is {name: 'ecs.capability.execution-role-awslogs'} — the docker-remote-api capability is never derived and the list order differs. Why: a client indexing into either list, or testing for a specific capability, reads the wrong value. Fix: RegisterTaskDefinition's compatibility/attribute derivation. Traces: ecs-fargate-task-definition-round-trips-awslogs-and-omits-entry-point#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1; all four clusters have empty owned_by and empty stub_path, status 200 on both sides.
Cause 1 — $.taskDefinition.requiresAttributes[name="com.amazonaws.ecs.capability.ecr-auth"], [name="com.amazonaws.ecs.capability.task-iam-role"] and [name="ecs.capability.execution-role-ecr-pull"] all 'missing'; compatibilities[1] expected 'MANAGED_INSTANCES', got 'FARGATE'; revision expected 1, got 4; taskDefinitionArn expected to end ':1', got ':4'.
Cause 2 — $.taskDefinition.executionRoleArn and $.taskDefinition.taskRoleArn 'missing'; same three requiresAttributes missing; compatibilities[1] mismatch.
Cause 3 — $.taskDefinition.requiresAttributes expected [{'name': 'com.amazonaws.ecs.capability.ecr-auth'}], got absent.
Cause 4 — $.taskDefinition.compatibilities[2] expected 'MANAGED_INSTANCES', got absent; requiresAttributes[2] expected {'name': 'com.amazonaws.ecs.capability.docker-remote-api.1.19'}, got {'name': 'ecs.capability.execution-role-awslogs'}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Representative: fixture ecs-ecracc-fargate-ecr-image-execution-role-vs-task-role, step task#6 — register a Fargate definition with an ECR image plus execution and task roles; diff taskDefinition.requiresAttributes, compatibilities and revision.
3. Replay each fixture#step in trace_ids and diff the taskDefinition body against the recorded response.
