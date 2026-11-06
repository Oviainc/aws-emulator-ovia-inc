Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|RunTask|2xx->2xx|$.tasks[*].attachments[*].details[*]
Issue-Key: ecs|RunTask|2xx->2xx|$.tasks[*].containers[*].lastStatus
Issue-Key: ecs|RunTask|2xx->2xx|$.tasks[*].containers[*].networkInterfaces[*]
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].attachments[*].details[*].value
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].containers[*].networkInterfaces[*].privateIpv4Address
Issue-Key: ecs|DescribeServices|0->None|$
Issue-Key: ecs|DescribeServices|0->None|status
Issue-Related: 20261103T150504Z-ecs-task-records-omit-platform-and-sizing-fields.md
Issue-Commit: dabd605678ea57cce68275edff8c08147ffd283a

# ECS Fargate tasks and deployments never advance: task records omit platform/sizing fields, publish ENI details too early, and never reach a terminal or rolled-back state

> _Note: this issue was identified against the code base at commit `dabd605678ea57cce68275edff8c08147ffd283a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Three net-new ECS root causes clustered by one shared fault: the emulator's ECS task/deployment state machine is static — a task is materialised with a partial Task record and then never transitions, so neither a task's terminal lifecycle nor a deployment's circuit-breaker outcome is ever observable. Each cause must be closed before this issue is submitted.

Cause 1 of 3: RunTask's Fargate Task record omits modelled platform/sizing/override fields and publishes ENI details that do not exist yet. Fixture ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record, step task#10.
AWS: Smithy (com.amazonaws.ecs) RunTask -> RunTaskResponse.tasks of shape Tasks -> Task. Task models platformVersion ("The platform version where your task runs on... If you didn't specify one, the LATEST platform version is used"), platformFamily ("The operating system that your tasks are running on. A platform family is specified only for tasks that use the Fargate launch type"), ephemeralStorage (EphemeralStorage), fargateEphemeralStorage (TaskEphemeralStorage), overrides (TaskOverride), attributes (Attributes), availabilityZone ("The Availability Zone for the task.") and attachments ("The Elastic Network Adapter that's associated with the task if the task uses the awsvpc network mode"). For a Fargate task AWS populates all of these in the RunTask response itself: platformFamily for the task definition's OS family, the resolved platformVersion (never left unset), ephemeralStorage and fargateEphemeralStorage carrying the task's ephemeral-storage size, overrides carrying one containerOverrides entry per container in the task definition plus an empty inferenceAcceleratorOverrides, an attributes entry for the resolved CPU architecture (name "ecs.cpu-architecture"), and the task's Availability Zone. At RunTask time the task is PROVISIONING, so its awsvpc attachment carries only the placement detail (subnetId) and its status is PRECREATED/ATTACHING, containers[].networkInterfaces is still empty, and containers[].lastStatus is PENDING (PROVISIONING is a task-level lastStatus, not a container one). Container identity is a dashed UUID in the container ARN's last segment.
Emulator: the response omits attributes, availabilityZone, overrides, platformFamily, platformVersion, ephemeralStorage and fargateEphemeralStorage entirely (all None). It simultaneously emits attachment details the recording does not have yet — networkInterfaceId, macAddress and privateIPv4Address — and a populated containers[].networkInterfaces entry, while reporting containers[0].lastStatus as 'PROVISIONING' instead of 'PENDING'. The container ARN's container segment is a 32-character zero-padded hex counter rather than a dashed UUID (the specific id value is per-run and not itself a requirement; the dashed-UUID form is).
Why: a client that reads platformVersion/platformFamily to decide platform compatibility, ephemeralStorage to size scratch space, or overrides to confirm what it submitted, gets nothing back and cannot distinguish "not set" from "not reported". A client that watches the attachment to learn when the ENI is ready sees an interface id, MAC and private IP for an interface that is not attached yet, so it proceeds against networking that does not exist; and a client keying off container lastStatus sees a task-level value it cannot map.
Fix: the ECS RunTask handler's task-materialisation and Task serializer in the ecs service crate — populate the modelled Fargate fields from the task definition and the resolved platform, build overrides from the task definition's container list, gate attachment detail emission on the attachment's lifecycle (subnetId only until the ENI attaches), and set container lastStatus from the container's own state.
Traces: ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record#task#10.

Cause 2 of 3: a Fargate task whose image tag cannot be pulled never reaches the STOPPED lifecycle. Fixture ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record, step task#16 (DescribeTasks).
AWS: Smithy DescribeTasks -> Task. When a Fargate task's image pull fails, AWS drives the task to the terminal state and records it: desiredStatus and lastStatus become 'STOPPED', stopCode (TaskStopCode) becomes 'TaskFailedToStart', stoppedReason carries the pull failure text, and the lifecycle timestamps pullStartedAt, pullStoppedAt, executionStoppedAt, stoppingAt and stoppedAt are set (Task models each: "The Unix timestamp for the time when the container image pull began/completed", "...task execution stopped", "...task transitions from the RUNNING state to STOPPING", "...transitioned from the RUNNING state to the STOPPED state"). The awsvpc attachment's status becomes 'DELETED' once the ENI is torn down, containers[].lastStatus becomes 'STOPPED' and containers[].runtimeId is reported. Task.tags and the platform/sizing fields of cause 1 are reported here too.
Emulator: the task is frozen in its launch state — lastStatus and desiredStatus stay 'RUNNING', containers[0].lastStatus stays 'RUNNING' with no runtimeId, the attachment stays 'ATTACHED' instead of 'DELETED', and stopCode, stoppedReason, stoppedAt, stoppingAt, executionStoppedAt, pullStartedAt, pullStoppedAt, tags, overrides, attributes, availabilityZone, platformFamily, platformVersion, ephemeralStorage and fargateEphemeralStorage are all absent.
Why: a client cannot tell a healthy task from one that failed to start. Every failure-handling path keyed on lastStatus=STOPPED plus stopCode=TaskFailedToStart (retry, alarm, replace, surface the pull error to a user) never fires, and a waiter on tasks_stopped blocks forever against a task that AWS would have terminated.
Fix: the ECS task state machine / task-lifecycle advance in the ecs service crate's state, plus the DescribeTasks serializer — model the pull-failure transition (PROVISIONING/PENDING -> STOPPED with stopCode TaskFailedToStart, stoppedReason, and the pull/stop timestamps), detach the ENI attachment to DELETED, and stop the task's containers. Open issues 20261103T150504Z-ecs-task-records-omit-platform-and-sizing-fields.md and 20261104T060239Z-ec2-ecr-ecs-a-fargate-task-that-cannot.md describe neighbouring symptoms but do not carry this fixture step's replay key, so they do not cover it.
Traces: ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record#task#16.

Cause 3 of 3: a deployment with the circuit breaker armed never settles, so a DescribeServices poll never terminates. Fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop, step task#15 (DescribeServices).
AWS: recorded behavior plus the modelled deployment surface — a service whose deploymentConfiguration.deploymentCircuitBreaker has enable=true (and rollback=true) reaches a terminal deployment outcome within minutes: repeated task launch failures trip the breaker, the primary deployment's rolloutState moves from IN_PROGRESS to FAILED with a rolloutStateReason, and with rollback enabled ECS starts a new deployment onto the last known-good task definition. A DescribeServices poll therefore converges and the step completes.
Emulator: nothing converges. Replay records no response at all for this step — "status: expected , got None" with "$: fixture budget 600s spent" — i.e. the poll on DescribeServices ran until the fixture's whole 600-second budget was consumed because the deployment stayed in its non-terminal state indefinitely. This is the emulator's non-convergence, not a runner defect: the request was served, the state it was waited on never changed.
Why: any client that deploys and then waits (a CI/CD rollout gate, the services_stable waiter, a rollback supervisor) hangs instead of being told the deployment failed and was rolled back; and the distinction the scenario exists to check — breaker rollback versus a plain stop — is unobservable.
Fix: the ECS deployment/rollout state machine in the ecs service crate's state — advance a deployment's rolloutState/stabilityStatus as tasks fail, trip deploymentCircuitBreaker at its threshold, set rolloutState FAILED with a reason, and create the rollback deployment when rollback is enabled. Open issues 20261106T115645Z-ecs-3-net-new-causes-a-cluster-scoped.md and 20261104T062603Z-ecs-rolling-deployment-controls-are-not-enforced-the.md describe the same area but neither carries this fixture step's replay key.
Traces: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop#task#15.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (score 82.1%, 200 mismatched steps).

Cause 1 — ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record#task#10, RunTask, expected 200 / actual 200, body diverges:
  $.tasks[0].attributes: missing (expected [{'name': 'ecs.cpu-architecture', 'value': 'x86_64'}], got None)
  $.tasks[0].availabilityZone: missing (expected <shaped-not-asserted>, got None)
  $.tasks[0].platformFamily: missing (expected 'Linux', got None)
  $.tasks[0].platformVersion: missing (expected '1.4.0', got None)
  $.tasks[0].ephemeralStorage: missing (expected {'sizeInGiB': 20}, got None)
  $.tasks[0].fargateEphemeralStorage: missing (expected {'sizeInGiB': 20}, got None)
  $.tasks[0].overrides: missing (expected {'containerOverrides': [{'name': 'app'}], 'inferenceAcceleratorOverrides': []}, got None)
  $.tasks[0].containers[0].lastStatus: value_mismatch (expected 'PENDING', got 'PROVISIONING')
  $.tasks[0].attachments[0].details[1..3]: extra (got networkInterfaceId / macAddress / privateIPv4Address where AWS has none yet)
  $.tasks[0].containers[0].networkInterfaces[0]: extra (got an attachment/private-IP entry where AWS has none yet)
  $.tasks[0].containers[0].containerArn: last segment is a 32-hex zero-padded counter instead of a dashed UUID
(The recorded AZ, platform version '1.4.0', ephemeral size 20 and container name 'app' are that recording's request/state values; the requirement is that the members are present and derived from the request and resolved platform.)

Cause 2 — ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record#task#16, DescribeTasks, expected 200 / actual 200:
  $.tasks[0].lastStatus: value_mismatch (expected 'STOPPED', got 'RUNNING')
  $.tasks[0].desiredStatus: value_mismatch (expected 'STOPPED', got 'RUNNING')
  $.tasks[0].containers[0].lastStatus: value_mismatch (expected 'STOPPED', got 'RUNNING')
  $.tasks[0].stopCode: missing (expected 'TaskFailedToStart', got None)
  $.tasks[0].stoppedReason / stoppedAt / stoppingAt / executionStoppedAt / pullStartedAt / pullStoppedAt: missing
  $.tasks[0].attachments[0].status: value_mismatch (expected 'DELETED', got 'ATTACHED')
  $.tasks[0].containers[0].runtimeId: missing
  plus the same platform/sizing/overrides/attributes/tags omissions as cause 1
(The recorded epoch timestamps are per-run values; the requirement is that the members are set when the transition happens.)

Cause 3 — ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop#task#15, DescribeServices:
  status: expected <none recorded>, got None
  $: fixture budget 600s spent
The step is a poll for the circuit-breaker outcome; the emulator served the reads but the deployment state never reached a terminal rolloutState, so the 600s fixture budget was exhausted with no settled response.

## Steps to Reproduce
1. Build and boot the emulator from this checkout; replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record to step task#10 — register a Fargate task definition and RunTask it with awsvpc networkConfiguration. Inspect tasks[0]: platformFamily, platformVersion, ephemeralStorage, fargateEphemeralStorage, overrides, attributes and availabilityZone are absent; the attachment already lists networkInterfaceId/macAddress/privateIPv4Address and containers[0].lastStatus is 'PROVISIONING'.
3. Cause 2: continue the same fixture to step task#16 — the task definition's image tag cannot be pulled. Call DescribeTasks on the task: lastStatus/desiredStatus stay 'RUNNING' and stopCode/stoppedReason/stoppedAt are never set; AWS reports STOPPED with stopCode 'TaskFailedToStart'.
4. Cause 3: replay fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop to step task#15 — create a service with deploymentCircuitBreaker enable=true, rollback=true whose tasks cannot start, then poll DescribeServices for a terminal deployment rolloutState. The poll never converges and the fixture's 600s budget is consumed; AWS reaches rolloutState FAILED and starts the rollback deployment.
