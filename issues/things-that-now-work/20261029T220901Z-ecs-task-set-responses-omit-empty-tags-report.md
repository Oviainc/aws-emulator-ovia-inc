Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-err-service-not-active
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-err-service-not-active ecs-task-set-external-deployment-workflow ecs-taskset-basic-lifecycle ecs-taskset-create-describe-update-delete-lifecycle ecs-taskset-crud-lifecycle ecs-taskset-errors ecs-taskset-lifecycle ecs-taskset-lifecycle-ec2 ecs-taskset-multiple-primary-switch ecs-taskset-primary-promotion ecs-taskset-tags-and-include-fields ecs-taskset-tags-and-external-id
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|CreateTaskSet|2xx->2xx|$.taskSet.stabilityStatus
Issue-Key: ecs|CreateTaskSet|2xx->2xx|$.taskSet.tags
Issue-Key: ecs|DescribeTaskSets|2xx->2xx|$.taskSets[id="ecs-svc/0000000000000000001"].serviceArn
Issue-Key: ecs|DescribeTaskSets|2xx->2xx|$.taskSets[id="ecs-svc/0000000000000000001"].tags
Issue-Key: ecs|DescribeTaskSets|2xx->2xx|$.taskSets[id="ecs-svc/0000000000000000002"].serviceArn
Issue-Key: ecs|DescribeTaskSets|2xx->2xx|$.taskSets[id="ecs-svc/0000000000000000002"].tags
Issue-Key: ecs|UpdateServicePrimaryTaskSet|2xx->2xx|$.taskSet.scale.value
Issue-Key: ecs|UpdateServicePrimaryTaskSet|2xx->2xx|$.taskSet.serviceArn
Issue-Key: ecs|UpdateServicePrimaryTaskSet|2xx->2xx|$.taskSet.stabilityStatus
Issue-Key: ecs|UpdateServicePrimaryTaskSet|2xx->2xx|$.taskSet.tags
Issue-Key: ecs|DescribeTaskSets|2xx->2xx|$.taskSets[id="ecs-svc/0000000000000000002"].scale.value
Issue-Key: ecs|DescribeTaskSets|2xx->2xx|$.taskSets[id="ecs-svc/0000000000000000001"].scale.value
Issue-Key: ecs|DescribeTaskSets|2xx->2xx|$.taskSets[id="ecs-svc/0000000000000000001"].stabilityStatus
Issue-Key: ecs|DescribeTaskSets|2xx->2xx|$.taskSets[id="ecs-svc/0000000000000000002"].stabilityStatus
Issue-Commit: 20487886de73e0d49e1ee92e88e94f424cdb9e47

# ECS task-set responses omit empty tags, report STEADY_STATE at creation, lose scale updates and synthesize a cluster-qualified serviceArn

> _Note: this issue was identified against the code base at commit `20487886de73e0d49e1ee92e88e94f424cdb9e47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTaskSet

## Description
Ten net-new root causes, all in how the emulator renders a TaskSet on the wire: the empty tags list is omitted, stabilityStatus is hard-wired to STEADY_STATE, an UpdateTaskSet scale change is not persisted, and serviceArn is re-derived as a cluster-qualified ARN that disagrees with the emulator's own service responses. All ten must be closed before submitting.

Cause 1 of 10: CreateTaskSet (fixture ecs-err-service-not-active, step task#5).
AWS: Smithy TaskSet.stabilityStatus targets StabilityStatus (values STEADY_STATE, STABILIZING) and its doc states STEADY_STATE requires runningCount == computedDesiredCount, pendingCount == 0, no tasks on DRAINING instances and all health checks passing; "If any of those conditions aren't met, the stability status returns STABILIZING". A task set just created has not converged, so AWS answers 200 with stabilityStatus STABILIZING. Smithy TaskSet.tags targets Tags (list, @length 0..50) and AWS serializes it as an empty array for an untagged task set.
Emulator: returns stabilityStatus STEADY_STATE and omits tags entirely.
Why: a client that polls stabilityStatus to wait for a deployment to converge sees STEADY_STATE on the very first response and proceeds against a task set with no running tasks; a client reading taskSet.tags gets an absent member instead of an empty list.
Fix: CreateTaskSet's handler (initial stability derivation) and the TaskSet serializer (always emit the tags list).

Cause 2 of 10: DescribeTaskSets (fixture ecs-task-set-external-deployment-workflow, step task#7).
AWS: recorded behavior only — TaskSet.serviceArn is documented only as "the ARN of the service the task set exists in". In the recording that ARN is the same ARN the service's own responses carry, of the form arn:aws:ecs:<region>:<account>:service/<service-name>, while the sibling members taskSetArn and clusterArn are cluster-qualified. The rule is that serviceArn must be identical to the service ARN returned by CreateService/DescribeServices, not re-derived.
Emulator: emits arn:aws:ecs:<region>:<account>:service/<cluster-name>/<service-name>, i.e. it prefixes the cluster segment inside the task-set serializer even though its own service responses use the short form (those steps pass). The same response also omits the empty tags list.
Why: a client that correlates a task set with its service by string-comparing serviceArn against the ARN it got from CreateService never matches, so task-set-to-service joins silently fail.
Fix: the TaskSet serializer / task-set state must carry the stored service ARN rather than compose one from cluster+service names.

Cause 3 of 10: UpdateServicePrimaryTaskSet (fixture ecs-task-set-external-deployment-workflow, step task#10).
AWS: the returned TaskSet reflects current state, including a scale previously changed by UpdateTaskSet (Scale.value is a floating-point percentage). In the recording the task set had been scaled and the response carries that updated percentage.
Emulator: returns the creation-time scale percentage, so the UpdateTaskSet mutation was never persisted; the same response also carries the cluster-qualified serviceArn, stabilityStatus STEADY_STATE instead of STABILIZING, and no tags list.
Why: a client that scales a task set and then reads it back sees its write silently discarded.
Fix: UpdateTaskSet's state mutation (persist scale) plus the shared TaskSet serializer.

Cause 4 of 10: DescribeTaskSets (fixture ecs-task-set-external-deployment-workflow, step task#11).
AWS: a subsequent read of the same task sets shows the scale set by UpdateTaskSet and the stored service ARN.
Emulator: scale is still the creation value and serviceArn is cluster-qualified; tags omitted.
Why: the lost scale write is observable on every later read, not just the mutating call.
Fix: task-set state (scale) and the TaskSet serializer.

Cause 5 of 10: DescribeTaskSets (fixture ecs-taskset-basic-lifecycle, step task#6).
AWS: serviceArn equals the service's stored ARN; tags is [].
Emulator: cluster-qualified serviceArn, tags omitted.
Why: as cause 2.
Fix: the TaskSet serializer.

Cause 6 of 10: DescribeTaskSets (fixture ecs-taskset-basic-lifecycle, step task#8).
AWS: after the task set is scaled to a new percentage, the read-back shows that percentage.
Emulator: shows the creation percentage, plus the cluster-qualified serviceArn and no tags.
Why: as cause 3.
Fix: UpdateTaskSet state mutation + serializer.

Cause 7 of 10: UpdateServicePrimaryTaskSet (fixture ecs-taskset-basic-lifecycle, step task#9).
AWS: the promoted task set is returned with its current scale, the stored serviceArn and tags [].
Emulator: stale scale, cluster-qualified serviceArn, tags omitted.
Why: as cause 3.
Fix: UpdateTaskSet state mutation + serializer.

Cause 8 of 10: DescribeTaskSets (fixture ecs-taskset-multiple-primary-switch, step task#7).
AWS: both task sets report stabilityStatus STABILIZING because they have not converged, serviceArn is the stored service ARN and tags is [].
Emulator: STEADY_STATE for both, cluster-qualified serviceArn, tags omitted.
Why: as cause 1 — a blue/green switch appears stable before it is.
Fix: task-set stability derivation + serializer.

Cause 9 of 10: UpdateServicePrimaryTaskSet (fixture ecs-taskset-multiple-primary-switch, step task#8).
AWS: promoting a task set returns stabilityStatus STABILIZING (promotion restarts convergence), the stored serviceArn and tags [].
Emulator: STEADY_STATE, cluster-qualified serviceArn, tags omitted.
Why: as cause 1.
Fix: UpdateServicePrimaryTaskSet handler's stability derivation + serializer.

Cause 10 of 10: CreateTaskSet (fixture ecs-taskset-tags-and-external-id, step task#5).
AWS: a newly created task set reports stabilityStatus STABILIZING.
Emulator: STEADY_STATE.
Why: as cause 1.
Fix: CreateTaskSet handler's stability derivation.

Related open issues, none of which covers these causes: "ecs: 7 net-new root causes in state and derivation" (20261029T175650Z-ecs-7-net-new-root-causes-in-state.md) and "Implement ecs DeleteTaskSet: unimplemented scaffold stub" (20261029T175259Z-ecs-deletetaskset-stub.md).

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, 670 steps, 185 mismatched):
- CreateTaskSet 200/200: $.taskSet.stabilityStatus expected 'STABILIZING', got 'STEADY_STATE'; $.taskSet.tags expected [], got absent (13 affected steps).
- DescribeTaskSets 200/200: $.taskSets[...].serviceArn expected 'arn:aws:ecs:<region>:<account>:service/<service>', got 'arn:aws:ecs:<region>:<account>:service/<cluster>/<service>'; $.taskSets[...].tags expected [], got absent (6 and 7 affected steps on two clusters).
- DescribeTaskSets / UpdateServicePrimaryTaskSet 200/200: $.taskSet(s)[...].scale.value expected the percentage set by the preceding UpdateTaskSet, got the creation percentage.
- UpdateServicePrimaryTaskSet 200/200: stabilityStatus expected 'STABILIZING', got 'STEADY_STATE' (4 affected steps).
Smithy (ECS 2014-11-13): TaskSet.stabilityStatus -> StabilityStatus enum [STEADY_STATE, STABILIZING]; TaskSet.tags -> Tags list @length 0..50; TaskSet.serviceArn -> String; CreateTaskSet / UpdateServicePrimaryTaskSet output member taskSet -> TaskSet.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus) and read the ecs clusters.
2. Or by hand: CreateCluster, RegisterTaskDefinition, CreateService with EXTERNAL deployment controller, then CreateTaskSet with scale {value:50,unit:PERCENT} and no tags -> response has stabilityStatus STEADY_STATE and no tags member.
3. UpdateTaskSet the task set to a different scale percentage, then DescribeTaskSets -> scale.value is still the creation value.
4. Compare taskSet.serviceArn with the serviceArn returned by CreateService/DescribeServices -> the task-set copy carries an extra /<cluster>/ segment.
5. UpdateServicePrimaryTaskSet -> stabilityStatus STEADY_STATE instead of STABILIZING.
