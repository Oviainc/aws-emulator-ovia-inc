Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-escc-traj-mirror-custom-service-create-refusals
Issue-Step: task#13
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-escc-traj-mirror-custom-service-create-refusals ecs-ecsnrt-traj-run-task-awsvpc-network-configuration-mistakes ecs-ecsnrt-traj-run-task-launch-selection-mistakes ecs-ecscm-traj-private-dns-srv-reference-path ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record ecs-ecsnrt-traj-task-listing-and-describe-mistakes
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|CreateService|2xx->4xx|$.service
Issue-Key: ecs|CreateService|2xx->4xx|status
Issue-Key: ecs|DeleteCluster|2xx->4xx|status
Issue-Key: ecs|DeleteService|2xx->4xx|status
Issue-Key: ecs|ListTasks|2xx->4xx|$.taskArns
Issue-Key: ecs|ListTasks|2xx->4xx|status
Issue-Commit: 34977689197ee7e7ec3a117f2c4937da4e6f0436

# ECS: the refusals of four request classes AWS answers are still open — one create probe the emulator still accepts, one leaked service, one listing refusal whose predicate is unidentified, and one fixture blocked in EC2 (4 of 6 causes)

> _Split out of `issues/things-that-now-work/20261103T150504Z-ecs-refuses-six-request-classes-aws-accepts-answering.md`, whose causes 4 (`ListServicesByNamespace` for a Cloud Map namespace with no ECS service in it) and 5 (`UpdateService` with a partial `deploymentConfiguration`) are fixed, and whose causes 1 and 2 are fixed for the step each was filed against (the bound step `ecs-ecscm-traj-create-service-launch-mistakes#task#11` is answered 200, and the `DeleteCluster` teardown passes in 2 of its 4 units). This report carries the replay keys that do NOT pass, re-filed here so they are not recorded as working. Everything below is what that fix established about them; a later fixer starts from here rather than from scratch._

## Operation
CreateService

## Description
Four of the parent report's six causes still have failing units. They are numbered as the parent numbered them, so the two reports can be read together. The parent's rule holds throughout: **a create the service refuses stores nothing, so the name it asked for stays free and the next, corrected create of that name is served** — every residue below is either a create the emulator still accepts and AWS refuses (which leaves a service behind and makes the next call fail), or a refusal whose predicate the replay does not expose.

Cause 1 of 6 (parent numbering), residue: one unit, `ecs-escc-traj-mirror-custom-service-create-refusals`. The parent fix made `CreateService` refuse a `capacityProviderStrategy` naming a provider the account holds none of, a `serviceRegistries` entry Cloud Map holds no service for, a `maximumPercent` below 100, a `bakeTimeInMinutes` outside 0..1440, a `taskDefinition` under a deployment controller that takes it from a task set, `assignPublicIp: ENABLED` off Fargate, and a retired Fargate platform version — which took this key from 3 failing units to 1. The remaining unit still holds a create the emulator ANSWERS and AWS REFUSES, so the corrected create later in that fixture is still refused as a duplicate name. Finding it needs the recorded request: `replay_issue_units` reports only each unit's FIRST divergence, and this unit's is the launch-type refusal WORDING (owned by `20261103T150504Z-ecs-validation-refusals-use-emulator-authored-wording-instead`), which hides the next one. Fix: identify the remaining accepted-but-refused create in that fixture (its step is after `#task#9`) and refuse it.

Cause 2 of 6 (parent numbering), residue: two units, `ecs-ecsnrt-traj-run-task-awsvpc-network-configuration-mistakes` and `ecs-ecsnrt-traj-run-task-launch-selection-mistakes`. AWS answers 200 with the Cluster body at the teardown's `DeleteCluster`; the emulator answers 400 `ClusterContainsServicesException` because a service AWS never created is still ACTIVE in the cluster. `DeleteCluster`'s own bookkeeping is correct (it counts only services that are neither DRAINING nor INACTIVE, and the parent fix pins the whole teardown end to end in `tests/all/accepted_request_classes.rs`), so what is left is the create that leaked the service — in these two fixtures, one of the `RunTask`/`CreateService` probes whose refusal the emulator does not reproduce. Fix: the same hunt as cause 1, in those fixtures.

Cause 3 of 6 (parent numbering): `DeleteService` at the teardown of `ecs-ecscm-traj-private-dns-srv-reference-path`. AWS: 200 with the Service body, the status moving toward INACTIVE. Emulator: 400. This unit's FIRST divergence is not in ECS at all: EC2 `DescribeVpcs` at `#setup#4` answers 400 where AWS answers 200, so the VPC the fixture's `awsvpc` service needs is never resolved, the service is never created, and the teardown's `DeleteService` has no service to delete. `DeleteService`'s handler itself is correct for the documented rule — "You can delete a service if you have no running tasks in it and the desired task count is zero", and `force` deletes one that was not scaled down — and both rows are pinned by the parent fix's suite. Fix: the EC2 `DescribeVpcs` divergence first (file or find its own issue), then re-replay this unit; nothing is known to be wrong in ECS for this cause.

Cause 6 of 6 (parent numbering): `ListTasks` filtered to stopped tasks, in `ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record#task#13` and `ecs-ecsnrt-traj-task-listing-and-describe-mistakes`. AWS: 200 with the matching `taskArns`. Emulator: 400 with no `taskArns`. What the parent fix established: the handler's `desiredStatus` path is NOT the fault. Its suite drives `ListTasks` over the whole `DesiredStatus` enum (RUNNING named and defaulted, PENDING, STOPPED) crossed with the cluster named and spelled as an ARN and with the `family` and `launchType` filters, after running two tasks and stopping one, and every row is answered 200 with exactly the matching ARNs. The refusal must therefore come from one of the handler's other predicates — `maxResults` outside 1..100, a `cluster` ARN of another account, a cluster that does not resolve (including the `default` cluster a request naming none assumes, which this store does not materialise), a `serviceName` that resolves to nothing, or a `containerInstance` that does — and which of them the recorded request trips is not visible, because both units diverge earlier on causes this issue does not own (the `RunTask` subnet-resolution message, and the task-record state machine in `20261103T150504Z-ecs-task-records-omit-platform-and-sizing-fields.md`). Fix: recover the recorded `ListTasks` request for those two steps, then close whichever predicate it trips. Guessing is not available: answering the call with a different refusal would be a re-worded refusal, not a fix.

## Evidence
Replay of trace set `simparity-wire-aws-v1` after the parent fix: 48 diverged steps across the 12 graded fixtures, down from 71, with `bound_step_state` for `ecs-ecscm-traj-create-service-launch-mistakes#task#11` now `not_among_divergences`.

* Cause 1 — key `ecs|CreateService|2xx->4xx|status` still_failing, units 3 -> 1 (`ecs-escc-traj-mirror-custom-service-create-refusals`).
* Cause 2 — key `ecs|DeleteCluster|2xx->4xx|status` still_failing, units 4 -> 2; actual `400 ClusterContainsServicesException: The Cluster cannot be deleted while Services are active.` against a recorded 200.
* Cause 3 — key `ecs|DeleteService|2xx->4xx|status` still_failing, 1 unit; that unit's first divergence is `DescribeVpcs (ecs-ecscm-traj-private-dns-srv-reference-path#setup#4)`, expected 200, actual 400.
* Cause 6 — keys `ecs|ListTasks|2xx->4xx|status` and `|$.taskArns` still_failing, 2 units; the recorded response carries a one-element `taskArns` list.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (`replay_wire_corpus`), or `replay_issue_units` for this report.
2. Causes 1 and 2: replay `ecs-escc-traj-mirror-custom-service-create-refusals` and the two `ecs-ecsnrt-traj-run-task-*` fixtures step by step and compare the status of EVERY `CreateService`/`RunTask` in them — the first one the emulator answers 200 and the recording refuses is the create that leaks the service.
3. Cause 3: replay `ecs-ecscm-traj-private-dns-srv-reference-path` from `#setup#4` and compare the EC2 `DescribeVpcs` response first.
4. Cause 6: replay `ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record#task#13` and read the REQUEST it sends beside the emulator's 400 body.
