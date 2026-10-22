Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 96234e472a4dbed875261b6f3ae0a9ed15fa6e7c

# Bootstrap the ECS emulator

_Filed: 2026-10-21T23:23:07Z_
_Operation: service-com-amazonaws-ecs-amazonec2containerservicev20141113 | Class: stub_

> _Note: this issue was identified against the code base at commit `96234e472a4dbed875261b6f3ae0a9ed15fa6e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 88 operations in `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the crate holds no state. No ECS client workflow works at all.

This issue scopes the FIRST buildable slice: the core task lifecycle of ECS, implemented statefully on the awsJson1_1 wire (`POST /`, `X-Amz-Target: AmazonEC2ContainerServiceV20141113.<Op>`). Implement exactly these six operations, in this order, so a realistic client script runs end to end:

1. **CreateCluster** — input `clusterName` (optional; defaults to `default`), `tags`, `settings`. Must persist a new cluster and return `cluster` (Cluster) with `clusterArn` (`arn:aws:ecs:<region>:<account>:cluster/<name>`), `clusterName`, `status=ACTIVE`, and zeroed counters (`registeredContainerInstancesCount`, `runningTasksCount`, `pendingTasksCount`, `activeServicesCount`), plus the `tags`/`settings` as given. A second CreateCluster with the same name must not silently create a duplicate entry.
2. **RegisterTaskDefinition** — input `family` (required), `containerDefinitions` (required), `networkMode`, `cpu`, `memory`, `volumes`, `requiresCompatibilities`, `tags`. Must assign the next `revision` per family (1, 2, 3 …), persist the definition, and return `taskDefinition` (TaskDefinition) with `taskDefinitionArn` = `arn:…:task-definition/<family>:<revision>`, `family`, `revision`, `status=ACTIVE`, `containerDefinitions` echoed back from the STORED record, and `registeredAt`.
3. **RunTask** — input `cluster` (name or ARN, defaults to `default`), `taskDefinition` (required; accept `family`, `family:revision`, or full ARN — bare family resolves to the latest ACTIVE revision), `count` (default 1), `launchType`, `startedBy`, `group`, `overrides`, `tags`. Must create `count` Task records in the named cluster and return them in `tasks`, each with a fresh `taskArn` (`arn:…:task/<cluster>/<uuid>`), `clusterArn`, `taskDefinitionArn`, `lastStatus`/`desiredStatus` (RUNNING is acceptable for the emulator), `containers` derived from the stored container definitions (each with `containerArn`, `name`, `taskArn`), `cpu`/`memory`, `createdAt`, `launchType`, `startedBy`, `group`. An unknown cluster must fail with `ClusterNotFoundException` (HTTP 400), not an empty 200. Running tasks must bump the cluster's `runningTasksCount`.
4. **DescribeTasks** — input `cluster`, `tasks` (required list of task ARNs or ids). Must return the STORED Task records for the ids that exist in `tasks`, and a `Failure{arn, reason:"MISSING"}` entry in `failures` for each id that does not. The task returned must be identical to the one RunTask created (same ARN, same taskDefinitionArn, same containers).
5. **StopTask** — input `cluster`, `task` (required), `reason`. Must mutate the stored task: `desiredStatus=STOPPED`, `lastStatus=STOPPED`, `stoppedReason` = the given reason (or the ECS default), `stoppingAt`/`stoppedAt` timestamps, and decrement the cluster's `runningTasksCount`. Returns the updated `task`. A subsequent DescribeTasks must show STOPPED; an unknown task id must fail (`InvalidParameterException`), not succeed.
6. **DeleteCluster** — input `cluster` (required). Must remove the cluster from state and return the final `cluster` with `status=INACTIVE`. A cluster that still has RUNNING tasks must be refused with `ClusterContainsTasksException` (HTTP 400). Deleting an unknown/already-deleted cluster must raise `ClusterNotFoundException`.

For list/read support of the slice, `ListTasks` (filters `cluster`, `family`, `startedBy`, `desiredStatus`; output `taskArns`) and `DescribeClusters` (output `clusters` + `failures` with `reason:"MISSING"`) should be derived from the same state so the workflow is inspectable, but the six operations above are the required core.

NEVER STUBS: every one of these must compute its response from persisted state. No canned bodies, no echoing the input back without storing it, no minting an ARN that is never recorded. A create followed by a describe must return what was created, and StopTask must be observable by a later DescribeTasks.

## State Model

In `src/state.rs` (`ComAmazonawsEcsAmazonec2containerservicev20141113State`, reached via `crate::state::service_state(&*self.engine)`):

- `clusters: HashMap<String /*clusterName*/, Cluster>` where `Cluster { name, arn, status: "ACTIVE"|"INACTIVE", tags: Vec<Tag>, settings: Vec<ClusterSetting>, configuration, capacity_providers: Vec<String>, default_capacity_provider_strategy, registered_container_instances_count: i32, running_tasks_count: i32, pending_tasks_count: i32, active_services_count: i32, statistics, created_at }`
- `task_definitions: HashMap<(String /*family*/, i32 /*revision*/), TaskDefinitionRecord>` plus `next_revision: HashMap<String /*family*/, i32>` so revisions increment per family; `TaskDefinitionRecord { arn, family, revision, status: ACTIVE|INACTIVE, container_definitions, network_mode, cpu, memory, volumes, requires_compatibilities, task_role_arn, execution_role_arn, tags, registered_at, deregistered_at }`
- `tasks: HashMap<String /*taskArn*/, TaskRecord>` where `TaskRecord { task_arn, cluster_arn, task_definition_arn, container_instance_arn, last_status, desired_status, containers: Vec<ContainerRecord{container_arn, name, task_arn, last_status, image}>, cpu, memory, overrides, launch_type, started_by, group, tags, created_at, started_at, stopping_at, stopped_at, stopped_reason, stop_code }`; index by cluster name for `ListTasks` filtering (or filter by `cluster_arn` on scan)
- Account/partition constants (account id, region) used to mint consistent ARNs across all six operations.

Operations outside this slice (services, task sets, capacity providers, container instances, daemons, gateways, attributes, account settings, tagging) stay 501 and are tracked as individual gap issues later.

## Evidence

OPERATIONS.md for this crate (88 operations, awsJson1_1, `X-Amz-Target: AmazonEC2ContainerServiceV20141113.<Op>`) documents the exact contracts cited: CreateCluster (line 162: input `clusterName`/`tags`/`settings`, output `cluster` (Cluster), errors incl. `InvalidParameterException`), DeleteCluster (line 143: input `cluster` required, errors incl. `ClusterContainsTasksException`, `ClusterNotFoundException`), RegisterTaskDefinition (line 526: `family` + `containerDefinitions` required, output `taskDefinition` + `tags`), RunTask (line 546: `taskDefinition` required, `cluster`, `count`, output `tasks` + `failures`, errors incl. `ClusterNotFoundException`), DescribeTasks (line 532: `tasks` required, output `tasks` + `failures`), StopTask (line 558: `task` required, `reason`, output `task`), ListTasks (line 239: paginated, output `taskArns`), DescribeClusters (line 174: output `clusters` + `failures`). Handler evidence: `src/ops/create_cluster.rs` lines 4 and 212 show the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateCluster"))`; all 88 files in `src/ops/` are the same generated stub, so no ECS behaviour exists and no test asserts any. Failure `reason: "MISSING"` for unknown ids and the ARN formats are well-established ECS behaviour.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI ECS client at it (`aws --endpoint-url <emulator> ecs ...`).
2. `CreateCluster{clusterName: "demo"}` → expect 200 with `cluster.clusterArn` ending in `cluster/demo`, `status=ACTIVE`; actual: HTTP 501 NotImplemented.
3. `RegisterTaskDefinition{family: "web", containerDefinitions: [{name: "app", image: "nginx", memory: 128}]}` → expect 200 with `taskDefinition.revision=1` and `taskDefinitionArn` ending in `task-definition/web:1`.
4. `RunTask{cluster: "demo", taskDefinition: "web", count: 1}` → expect 200 with one task whose `taskArn` is new, `lastStatus=RUNNING`, `containers[0].name="app"`.
5. `DescribeTasks{cluster: "demo", tasks: [<taskArn from step 4>]}` → expect the same task record back, `failures` empty; then `DescribeTasks{cluster:"demo", tasks:["bogus"]}` → expect `failures[0].reason="MISSING"`.
6. `StopTask{cluster: "demo", task: <taskArn>, reason: "done"}` → expect `task.desiredStatus=STOPPED` and `stoppedReason="done"`; re-run DescribeTasks and expect `lastStatus=STOPPED`.
7. `DeleteCluster{cluster: "demo"}` → expect 200 with `cluster.status=INACTIVE`; then `DescribeClusters{clusters:["demo"]}` → expect `failures[0].reason="MISSING"`, and a second `DeleteCluster` → `ClusterNotFoundException` (HTTP 400).
8. Also verify DeleteCluster is refused with `ClusterContainsTasksException` while a RUNNING task exists in the cluster.
