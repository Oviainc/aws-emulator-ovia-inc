Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-fargate-run-task-reports-container-exit-code
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-fargate-run-task-reports-container-exit-code
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|RunTask|2xx->2xx|$.tasks[*].createdAt
Issue-Key: ecs|RunTask|2xx->2xx|$.tasks[*].provisionedCpu
Issue-Key: ecs|RunTask|2xx->2xx|$.tasks[*].provisionedMemory
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].containers[*].exitCode
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].containers[*].imageDigest
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].createdAt
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].provisionedCpu
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].provisionedMemory
Issue-Related: 20261103T150504Z-ecs-task-records-omit-platform-and-sizing-fields.md
Issue-Related: 20261106T135659Z-ecs-fargate-tasks-and-deployments-never-advance-task.md
Issue-Commit: 54b3fe10738e925f02de63999e233b2f22231166

# ECS Fargate RunTask and DescribeTasks: task records omit platform/sizing fields, publish ENI details too early, and the task never reaches its STOPPED exit-code state

> _Note: this issue was identified against the code base at commit `54b3fe10738e925f02de63999e233b2f22231166`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Two net-new ECS wire causes, bundled because they share one fault: a Fargate task is materialised as a partial, static Task record — the modelled platform/sizing/override members are never populated, ENI details are published before the attachment exists, and the record never advances to the terminal state that carries the container's exit code. Both causes must be closed before this issue is submitted.

NOTE ON A NEIGHBOURING OPEN ISSUE: 20261106T135659Z-ecs-fargate-tasks-and-deployments-never-advance-task.md describes the same class of defect on a different fixture (ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record, an unpullable image tag). It carries neither of the replay keys below — this fixture runs a container that exits with a non-zero code — so these causes are net-new and must be re-replayed on their own.

Cause 1 of 2: RunTask's Fargate Task record omits the modelled platform, sizing, override and attribute members and already reports ENI details (operation RunTask; fixture ecs-fargate-run-task-reports-container-exit-code, step task#11).
AWS: Smithy com.amazonaws.ecs RunTask -> RunTaskResponse.tasks (Tasks -> Task). Task models platformVersion ("The platform version where your task runs on. A platform version is only specified for tasks that use the Fargate launch type. If you didn't specify one, the LATEST platform version is used"), platformFamily ("The operating system that your tasks are running on. A platform family is specified only for tasks that use the Fargate launch type"), ephemeralStorage (EphemeralStorage, "The ephemeral storage settings for the task"), fargateEphemeralStorage (TaskEphemeralStorage, "The Fargate ephemeral storage settings for the task"), overrides (TaskOverride, "One or more container overrides"), attributes (Attributes, "The attributes of the task"), availabilityZone ("The Availability Zone for the task") and attachments ("The Elastic Network Adapter that's associated with the task if the task uses the awsvpc network mode"). AWS populates all of these in the RunTask response for a Fargate task: the resolved platformVersion and platformFamily, ephemeralStorage/fargateEphemeralStorage carrying the task's ephemeral-storage size, overrides with one containerOverrides entry per container in the task definition plus an empty inferenceAcceleratorOverrides, an attributes entry named ecs.cpu-architecture for the resolved CPU architecture, and the task's Availability Zone. provisionedCpu and provisionedMemory are recorded behavior only (they are not in this checkout's Task shape) — the recording shows them present and equal to the task's resolved cpu/memory strings. At RunTask time the task has not been placed on an ENI yet, so its awsvpc attachment carries only the subnetId placement detail and containers[].networkInterfaces is empty; the container ARN's last segment is a dashed UUID.
Emulator: attributes, availabilityZone, ephemeralStorage, fargateEphemeralStorage, overrides, platformFamily, platformVersion, provisionedCpu and provisionedMemory are all absent (None), while the attachment ALREADY lists networkInterfaceId, macAddress and privateIPv4Address details and containers[0].networkInterfaces already carries an {attachmentId, privateIpv4Address} entry. The container ARN's last segment is a 32-character zero-padded hex counter rather than a dashed UUID (the specific value is per-run; the form is the requirement).
Why: a client that reads platformVersion/platformFamily for platform compatibility, ephemeralStorage to size scratch space, attributes for the CPU architecture or overrides to confirm what it submitted gets nothing back and cannot distinguish "not set" from "not reported". A client watching the attachment to learn when the ENI is ready is handed an interface id, MAC and private IP for an interface that does not exist yet, so it proceeds against networking that is not there.
Fix: the ECS RunTask handler's task materialisation and the Task serializer in the ecs service crate — populate the modelled Fargate members from the task definition and the resolved platform, build overrides from the task definition's container list, gate attachment-detail emission on the attachment's own lifecycle (subnetId only until the ENI attaches), and mint the container id in the dashed-UUID form.
Traces: ecs-fargate-run-task-reports-container-exit-code#task#11.

Cause 2 of 2: a Fargate task whose essential container exits never reaches STOPPED, so DescribeTasks never reports the exit code and the poll burns its whole budget (operation DescribeTasks; fixture ecs-fargate-run-task-reports-container-exit-code, step task#12).
AWS: Smithy DescribeTasks -> Task. When a task's essential container exits, AWS drives the task to its terminal state: lastStatus and desiredStatus become STOPPED, stopCode (TaskStopCode) becomes EssentialContainerExited, stoppedReason carries the reason text, the lifecycle timestamps pullStartedAt, pullStoppedAt, startedAt, executionStoppedAt, stoppingAt and stoppedAt are set (Task documents each, e.g. "The Unix timestamp for the time when the task execution stopped" and "...transitioned from the RUNNING state to the STOPPED state"), containers[].lastStatus becomes STOPPED with the container's exitCode (the recording's container exits 3), imageDigest and runtimeId reported, and the awsvpc attachment's status becomes DELETED once the ENI is torn down. The Task's tags, attributes, availabilityZone, overrides, platform and ephemeral-storage members are reported here too.
Emulator: the task is frozen in its launch state. The poll for the terminal record is capped at 180s and the record still diverges on lastStatus, desiredStatus, containers[0].lastStatus, containers[0].exitCode, containers[0].imageDigest, containers[0].runtimeId, stopCode, stoppedReason, stoppedAt, stoppingAt, executionStoppedAt, pullStartedAt, pullStoppedAt, connectivityAt, the attachment's status (ATTACHED instead of DELETED) and the same omitted platform/sizing/override/attribute members as cause 1.
Why: a client cannot tell a healthy task from one whose container exited non-zero. The exit code is the whole point of a one-shot task: every path keyed on lastStatus=STOPPED plus stopCode/exitCode (retry, alarm, report the failure to a user) never fires, and a tasks-stopped waiter blocks forever against a task AWS would have terminated minutes earlier.
Fix: the ECS task state machine / task-lifecycle advance in the ecs service crate's state, plus the DescribeTasks serializer — model the container-exit transition (RUNNING -> STOPPING -> STOPPED with stopCode EssentialContainerExited, stoppedReason, the pull/start/stop timestamps and the container's exitCode/runtimeId) and tear the ENI attachment down to DELETED.
Traces: ecs-fargate-run-task-reports-container-exit-code#task#12.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 118 fixtures, 1380 steps, 1206 passed, score 87.4%, 89 divergence clusters. Neither cluster below is blocked by an unimplemented-operation stub or owned by an open issue's replay key.

Cause 1 — ecs-fargate-run-task-reports-container-exit-code#task#11, RunTask, expected 200 / actual 200:
  $.tasks[0].attributes: missing (expected [{'name': 'ecs.cpu-architecture', 'value': 'x86_64'}], got None)
  $.tasks[0].availabilityZone: missing (expected 'us-west-2a', got None)
  $.tasks[0].ephemeralStorage: missing (expected {'sizeInGiB': 20}, got None)
  $.tasks[0].fargateEphemeralStorage: missing (expected {'sizeInGiB': 20}, got None)
  $.tasks[0].overrides: missing (expected {'containerOverrides': [{'name': '<container-name>'}], 'inferenceAcceleratorOverrides': []}, got None)
  $.tasks[0].platformFamily: missing (expected 'Linux', got None)
  $.tasks[0].platformVersion: missing (expected '1.4.0', got None)
  $.tasks[0].provisionedCpu: missing (expected '256', got None)
  $.tasks[0].provisionedMemory: missing (expected '512', got None)
  $.tasks[0].attachments[0].details[1..3]: extra (got {'name': 'networkInterfaceId', ...}, {'name': 'macAddress', ...}, {'name': 'privateIPv4Address', ...} where AWS has only subnetId at this point)
  $.tasks[0].containers[0].networkInterfaces[0]: extra (got {'attachmentId': ..., 'privateIpv4Address': ...}, expected none yet)
  $.tasks[0].containers[0].containerArn: last segment is a 32-char zero-padded hex counter instead of a dashed UUID

Cause 2 — ecs-fargate-run-task-reports-container-exit-code#task#12, DescribeTasks, expected 200 / actual 200, poll capped at 180s. Diverging keys: $.tasks[0].lastStatus, $.tasks[0].desiredStatus (expected 'STOPPED'), $.tasks[0].stopCode (expected 'EssentialContainerExited'), $.tasks[0].stoppedReason, $.tasks[0].stoppedAt, $.tasks[0].stoppingAt, $.tasks[0].executionStoppedAt, $.tasks[0].pullStartedAt, $.tasks[0].pullStoppedAt, $.tasks[0].startedAt, $.tasks[0].connectivityAt, $.tasks[0].containers[0].lastStatus (expected 'STOPPED'), $.tasks[0].containers[0].exitCode (expected 3), $.tasks[0].containers[0].imageDigest, $.tasks[0].containers[0].runtimeId, $.tasks[0].attachments[0].status (expected 'DELETED'), $.tasks[0].tags, plus the same attributes/availabilityZone/overrides/platformFamily/platformVersion/ephemeralStorage/fargateEphemeralStorage/provisionedCpu/provisionedMemory omissions as cause 1.

The recorded AZ, platform version '1.4.0', ephemeral size 20, cpu/memory strings, container name, exit code 3 and epoch timestamps are that recording's request/state values; the requirement is that the members are present, derived from the request and the resolved platform, and that the terminal transition happens and is reported.

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture ecs-fargate-run-task-reports-container-exit-code to step task#11 — create a cluster, register a FARGATE/awsvpc task definition whose container exits with a non-zero code, then RunTask it with an awsvpcConfiguration. Inspect tasks[0]: attributes, availabilityZone, ephemeralStorage, fargateEphemeralStorage, overrides, platformFamily, platformVersion, provisionedCpu and provisionedMemory are absent, while the attachment already lists networkInterfaceId/macAddress/privateIPv4Address and containers[0].networkInterfaces is already populated.
3. Cause 2: continue the same fixture to step task#12 — poll DescribeTasks on that task for the terminal record. The task stays in its launch state: lastStatus/desiredStatus never become STOPPED, containers[0].exitCode and stopCode/stoppedReason/stoppedAt are never set and the attachment stays ATTACHED, so the poll is capped at 180s. AWS reports lastStatus STOPPED, stopCode EssentialContainerExited and the container's exit code.
