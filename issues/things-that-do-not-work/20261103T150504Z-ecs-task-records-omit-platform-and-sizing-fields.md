Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record ecs-ecsnrt-traj-run-task-launch-selection-mistakes
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|RunTask|2xx->2xx|$.tasks[*].attributes
Issue-Key: ecs|RunTask|2xx->2xx|$.tasks[*].availabilityZone
Issue-Key: ecs|RunTask|2xx->2xx|$.tasks[*].containers[*].containerArn
Issue-Key: ecs|RunTask|2xx->2xx|$.tasks[*].containers[*].networkInterfaces
Issue-Key: ecs|RunTask|2xx->2xx|$.tasks[*].ephemeralStorage
Issue-Key: ecs|RunTask|2xx->2xx|$.tasks[*].fargateEphemeralStorage
Issue-Key: ecs|RunTask|2xx->2xx|$.tasks[*].lastStatus
Issue-Key: ecs|RunTask|2xx->2xx|$.tasks[*].overrides
Issue-Key: ecs|RunTask|2xx->2xx|$.tasks[*].platformFamily
Issue-Key: ecs|RunTask|2xx->2xx|$.tasks[*].platformVersion
Issue-Key: ecs|DescribeTasks|2xx->2xx|$
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].attachments[*].details[*]
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].attachments[*].status
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].attributes
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].availabilityZone
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].connectivity
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].connectivityAt
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].containers[*].containerArn
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].containers[*].lastStatus
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].containers[*].networkInterfaces
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].containers[*].runtimeId
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].desiredStatus
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].ephemeralStorage
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].executionStoppedAt
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].fargateEphemeralStorage
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].lastStatus
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].overrides
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].platformFamily
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].platformVersion
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].pullStartedAt
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].pullStoppedAt
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].startedAt
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].stopCode
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].stoppedAt
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].stoppedReason
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].stoppingAt
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].tags
Issue-Key: ecs|StopTask|2xx->2xx|$.task.attachments[*]
Issue-Key: ecs|StopTask|2xx->2xx|$.task.availabilityZone
Issue-Key: ecs|StopTask|2xx->2xx|$.task.connectivity
Issue-Key: ecs|StopTask|2xx->2xx|$.task.connectivityAt
Issue-Key: ecs|StopTask|2xx->2xx|$.task.containers[*]
Issue-Key: ecs|StopTask|2xx->2xx|$.task.ephemeralStorage
Issue-Key: ecs|StopTask|2xx->2xx|$.task.executionStoppedAt
Issue-Key: ecs|StopTask|2xx->2xx|$.task.fargateEphemeralStorage
Issue-Key: ecs|StopTask|2xx->2xx|$.task.lastStatus
Issue-Key: ecs|StopTask|2xx->2xx|$.task.overrides
Issue-Key: ecs|StopTask|2xx->2xx|$.task.platformFamily
Issue-Key: ecs|StopTask|2xx->2xx|$.task.platformVersion
Issue-Key: ecs|StopTask|2xx->2xx|$.task.pullStartedAt
Issue-Key: ecs|StopTask|2xx->2xx|$.task.pullStoppedAt
Issue-Key: ecs|StopTask|2xx->2xx|$.task.stopCode
Issue-Key: ecs|StopTask|2xx->2xx|$.task.stoppedAt
Issue-Key: ecs|StopTask|2xx->2xx|$.task.stoppedReason
Issue-Key: ecs|PutClusterCapacityProviders|2xx->2xx|$.cluster.pendingTasksCount
Issue-Key: ecs|DescribeClusters|2xx->2xx|$.clusters[*].runningTasksCount
Issue-Key: ecs|DescribeClusters|2xx->2xx|$.clusters[*].pendingTasksCount
Issue-Commit: 34977689197ee7e7ec3a117f2c4937da4e6f0436

# ECS task records omit platform and sizing fields and never reach the stopped lifecycle

> _Note: this issue was identified against the code base at commit `34977689197ee7e7ec3a117f2c4937da4e6f0436`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Seven net-new causes, one fault: the Task record the emulator builds is structurally thinner than AWS's and its lifecycle never advances, and the cluster counters derived from tasks are consequently wrong. No open issue carries these replay keys.

Cause 1 of 7: RunTask's returned Task omits platform and sizing members and starts in the wrong state. AWS: tasks[0] carries attributes [{name 'ecs.cpu-architecture', value 'x86_64'}], availabilityZone, platformFamily 'Linux', platformVersion '1.4.0' (the RESOLVED version, not the 'LATEST' alias), ephemeralStorage {sizeInGiB 20} and fargateEphemeralStorage {sizeInGiB 20} for a Fargate task, overrides {containerOverrides: [{name: <container name>}], inferenceAcceleratorOverrides: []}, containers[0].networkInterfaces [], and lastStatus 'PROVISIONING' at creation. The constant tokens ('x86_64', 'Linux', 'PROVISIONING', the 20 GiB Fargate default) are fixed; names and ARNs come from the request. Emulator: all of those members are absent and lastStatus is 'PENDING'. Why: a client reading platformVersion/ephemeralStorage to decide behavior sees nothing, and one waiting through PROVISIONING -> PENDING -> RUNNING misses a state. Fix: RunTask's handler and the task state it writes, plus the Task serializer. Traces: ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record#task#10.

Cause 2 of 7: a task whose image cannot be pulled never stops. AWS: DescribeTasks converges on lastStatus 'STOPPED', desiredStatus 'STOPPED', stopCode 'TaskFailedToStart', stoppedReason the CannotPullContainerError sentence, connectivity 'CONNECTED', the ENI attachment status 'DELETED' with networkInterfaceId/macAddress/privateIPv4Address details, and pull/execution timestamps. Emulator: the poll is capped at 60s and the task stays RUNNING with none of those members. Why: an image-pull failure is indistinguishable from a healthy task, so a client waits forever instead of surfacing the pull error. Fix: the task state machine (pull/start failure path), not the serializer. Traces: ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record#task#11.

Cause 3 of 7: DescribeTasks for that task reports RUNNING with an ENI attachment stuck at 'PRECREATED' and only a subnetId detail. AWS: attachment status 'DELETED' with four details, containers[0].lastStatus 'STOPPED', runtimeId present, connectivity/connectivityAt set. Emulator: attachment 'PRECREATED', container lastStatus 'RUNNING', runtimeId/connectivity absent. Why: the network attachment lifecycle is never driven, so a client cannot resolve the task's ENI. Fix: the task's attachment state transitions plus the Task serializer. Traces: ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record#task#16.

Cause 4 of 7: StopTask reports the wrong terminal record. AWS: the returned task has lastStatus 'STOPPED', stopCode 'TaskFailedToStart' with the pull-failure stoppedReason preserved (the task had ALREADY failed), no synthesized ENI attachment, and the platform/ephemeral-storage members present. Emulator: lastStatus 'DEACTIVATING', stopCode 'UserInitiated', an extra 'PRECREATED' attachment and an extra container entry, and no platform/storage members. Why: the reason a task stopped is overwritten by 'UserInitiated', destroying the diagnostic a client needs; and the task is not terminal when StopTask returns. Fix: StopTask's handler — preserve an existing terminal stop reason and settle to STOPPED. Traces: ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record#task#17.

Cause 5 of 7: PutClusterCapacityProviders echoes a nonzero pendingTasksCount. AWS: cluster.pendingTasksCount 0 — the tasks that failed placement are not pending. Emulator: 2. Why: a client gating on pendingTasksCount == 0 to decide a cluster is quiescent never proceeds. Fix: the cluster counters derived from task state. Traces: ecs-ecsnrt-traj-run-task-launch-selection-mistakes#task#12.

Cause 6 of 7: DescribeClusters reports runningTasksCount 2 where AWS reports 0, for tasks that never ran. Fix: the same cluster counters. Traces: ecs-ecsnrt-traj-run-task-launch-selection-mistakes#task#13.

Cause 7 of 7: DescribeClusters reports pendingTasksCount 1 and runningTasksCount 2 where AWS reports 0 and 0. Why: capacity dashboards and scale-out decisions built on these counters are wrong in the common 'placement failed' case. Fix: the same cluster counters. Traces: ecs-ecsnrt-traj-run-task-launch-selection-mistakes#task#15.

## Evidence
Replay of trace set simparity-wire-aws-v1; all seven clusters have empty owned_by and empty stub_path, status 200 on both sides.
Cause 1 — $.tasks[0].attributes / availabilityZone / containers[0].networkInterfaces / ephemeralStorage / fargateEphemeralStorage / overrides / platformFamily / platformVersion all 'missing'; $.tasks[0].lastStatus expected 'PROVISIONING', got 'PENDING'.
Cause 2 — '$: poll capped at 60s'; diverging keys include $.tasks[0].lastStatus, desiredStatus, stopCode, stoppedReason, connectivity, pullStartedAt, pullStoppedAt, attachments[0].status.
Cause 3 — $.tasks[0].attachments[0].status expected 'DELETED', got 'PRECREATED'; details[1..3] missing; containers[0].lastStatus expected 'STOPPED', got 'RUNNING'.
Cause 4 — $.task.lastStatus expected 'STOPPED', got 'DEACTIVATING'; $.task.stopCode expected 'TaskFailedToStart', got 'UserInitiated'; $.task.stoppedReason expected the CannotPullContainerError sentence; attachments[0] and containers[0] 'extra'.
Cause 5 — $.cluster.pendingTasksCount expected 0, got 2.
Cause 6 — $.clusters[0].runningTasksCount expected 0, got 2.
Cause 7 — $.clusters[0].pendingTasksCount expected 0, got 1; runningTasksCount expected 0, got 2.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Representative: fixture ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record, step task#10 — RunTask a Fargate task and diff tasks[0] against the recorded body.
3. Replay each fixture#step in trace_ids; the lifecycle causes are the steps whose detail reads 'poll capped at 60s'.
