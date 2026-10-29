Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-task-set-external-deployment-workflow
Issue-Step: task#16
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-task-set-external-deployment-workflow ecs-taskset-multiple-primary-switch ecs-taskset-primary-promotion ecs-taskset-basic-lifecycle ecs-taskset-tags-and-external-id ecs-taskset-create-describe-update-delete-lifecycle ecs-taskset-crud-lifecycle ecs-taskset-errors ecs-taskset-lifecycle ecs-taskset-tags-and-include-fields ecs-taskset-lifecycle-ec2
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DescribeTaskSets|2xx->2xx|$
Issue-Key: ecs|DescribeTaskSets|2xx->2xx|$.failures[*]
Issue-Key: ecs|DescribeTaskSets|2xx->2xx|$.taskSets
Issue-Key: ecs|DescribeTaskSets|2xx->2xx|$.taskSets[*]
Issue-Key: ecs|DescribeTaskSets|2xx->2xx|$.failures[*].reason
Issue-Key: ecs|DeleteService|2xx->2xx|$.service.taskSets[*]
Issue-Key: ecs|DeleteService|2xx->2xx|$.service.runStatus
Issue-Key: ecs|DeleteService|2xx->2xx|$.service.taskSets[*].scale.value
Issue-Key: ecs|DeleteService|2xx->2xx|$.service.taskSets[*].serviceArn
Issue-Key: ecs|DeleteService|2xx->2xx|$.service.taskSets[*].tags
Issue-Related: 20261029T220901Z-ecs-task-set-responses-omit-empty-tags-report.md
Issue-Commit: 20487886de73e0d49e1ee92e88e94f424cdb9e47

# ECS task-set deletion is not reflected: deleted task sets remain visible and DeleteService keeps returning them

> _Note: this issue was identified against the code base at commit `20487886de73e0d49e1ee92e88e94f424cdb9e47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTaskSets

## Description
Six net-new root causes around task-set removal: after the task sets are deleted the emulator keeps serving them, and the service teardown responses keep embedding them. All six must be closed before submitting.

Cause 1 of 6: DescribeTaskSets (fixture ecs-task-set-external-deployment-workflow, step task#16).
AWS: once a task set is deleted, DescribeTaskSets for its ARN answers 200 with taskSets: [] and one failures entry per requested identifier, each {"arn": <the task-set ARN as requested>, "reason": "MISSING"} — "MISSING" is fixed Failure.reason text, the arn is the value from the request. Smithy: DescribeTaskSets output carries taskSets (TaskSets) and failures (Failures).
Emulator: keeps returning both task sets with status ACTIVE/PRIMARY and failures: [], for the entire 60s poll window the recording allows.
Why: a client that deletes a task set and polls until it disappears never terminates, and a deployment-cleanup loop treats the deleted task set as live.
Fix: the task-set state store must remove (or mark terminal) the task set on DeleteTaskSet, and DescribeTaskSets must report unknown identifiers through failures rather than resolving them. Note DeleteTaskSet is an unimplemented stub tracked by 20261029T175259Z-ecs-deletetaskset-stub.md; these clusters are the observable wire effect on DescribeTaskSets/DeleteService and are not covered by that issue's replay key.

Cause 2 of 6: DescribeTaskSets (fixture ecs-taskset-basic-lifecycle, step task#12).
AWS: after the single task set is deleted, taskSets is [] and failures carries {arn, reason "MISSING"} for the requested task-set ARN.
Emulator: still returns the task set (status PRIMARY) and failures: [].
Why: as cause 1.
Fix: task-set state removal + DescribeTaskSets failure synthesis.

Cause 3 of 6: DescribeTaskSets (fixture ecs-taskset-create-describe-update-delete-lifecycle, step task#10).
AWS: same post-deletion shape — empty taskSets, one MISSING failure.
Emulator: still returns the task set with status ACTIVE and no failures.
Why: as cause 1.
Fix: as cause 1.

Cause 4 of 6: DeleteService (fixture ecs-taskset-basic-lifecycle, step teardown#13).
AWS: 200 with service.taskSets empty once the service's task sets have been deleted.
Emulator: service.taskSets still contains an entry (status DRAINING, stabilityStatus STABILIZING) for the deleted task set.
Why: a teardown that asserts the service was removed cleanly sees a lingering task set and cannot tell a leaked task set from a normal one.
Fix: DeleteService's service projection must read the (now-empty) task-set collection from state.

Cause 5 of 6: DeleteService (fixture ecs-taskset-lifecycle-ec2, step teardown#12).
AWS: 200 with service.runStatus 'SteadyState' (recorded behavior only — runStatus is not in the Smithy Service shape) and, where a task set is still projected, its scale percentage is the scaled-down value reached before teardown; serviceArn on that task set is the service's stored ARN and tags is [].
Emulator: service.runStatus 'MoreWork', the embedded task set reports the creation scale percentage, a cluster-qualified serviceArn and no tags member.
Why: a client that waits for runStatus to settle after DeleteService never observes the terminal value, and the scale/ARN mismatches mislead cleanup logic.
Fix: DeleteService handler (runStatus derivation) and the task-set projection it embeds.

Cause 6 of 6: DescribeTaskSets (fixture ecs-taskset-tags-and-external-id, step task#6).
AWS: taskSet.serviceArn is the ARN the service's own responses carry, arn:aws:ecs:<region>:<account>:service/<service-name>.
Emulator: arn:aws:ecs:<region>:<account>:service/<cluster-name>/<service-name>.
Why: task-set-to-service correlation by ARN fails.
Fix: the TaskSet serializer must emit the stored service ARN.

## Evidence
Replay of simparity-wire-aws-v1:
- DescribeTaskSets 200/200 after deletion, '$: poll capped at 60s': expected {'taskSets': [], 'failures': [{'arn': '<task-set arn>', 'reason': 'MISSING'}, ...]}, got the full task-set objects with 'failures': [] (clusters with 3, 2 and 5 affected steps).
- DeleteService 200/200: $.service.taskSets[0] extra — a DRAINING task set the recording does not contain.
- DeleteService 200/200: $.service.runStatus expected 'SteadyState', got 'MoreWork'; $.service.taskSets[0].scale.value expected 0.0, got 100.0; taskSets[0].serviceArn cluster-qualified; taskSets[0].tags absent.
- DescribeTaskSets 200/200: $.taskSets[...].serviceArn expected service/<service>, got service/<cluster>/<service>.
Smithy (ECS 2014-11-13): DescribeTaskSets output {taskSets: TaskSets, failures: Failures}; Failure {arn, reason, detail}.

## Steps to Reproduce
1. Replay the wire corpus (replay_wire_corpus) and read the ecs DescribeTaskSets/DeleteService clusters.
2. Or by hand: create cluster + task definition + EXTERNAL-controller service + task set; DeleteTaskSet it; then DescribeTaskSets with the task set's ARN -> the emulator still returns the task set instead of taskSets: [] plus a failures entry {arn, reason: 'MISSING'}.
3. DeleteService the service -> the response still embeds the deleted task set and reports runStatus 'MoreWork'.
