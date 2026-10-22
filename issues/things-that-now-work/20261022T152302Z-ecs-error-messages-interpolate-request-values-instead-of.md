Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-container-instance-lifecycle
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-container-instance-lifecycle ecs-fargate-awsvpc-task-definition-rendered-defaults ecs-run-task-and-task-protection ecs-task-plane-reads-and-launch-errors ecs-service-lifecycle-on-cluster ecs-task-definition-lifecycle ecs-task-definition-revision-lifecycle
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|RegisterContainerInstance|4xx->4xx|$.message
Issue-Key: ecs|RegisterTaskDefinition|4xx->4xx|$.message
Issue-Key: ecs|StopTask|4xx->4xx|$.message
Issue-Key: ecs|RunTask|4xx->4xx|$.message
Issue-Key: ecs|CreateService|4xx->4xx|$.message
Issue-Key: ecs|UpdateService|4xx->4xx|$.message
Issue-Key: ecs|DescribeTaskDefinition|4xx->4xx|$.message
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# ECS error messages interpolate request values instead of the fixed AWS sentences

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
On these paths the emulator gets the HTTP status (400) and the condition right, but builds its own message text: it interpolates the cluster/service/task/task-definition name into a bespoke lower-case sentence where ECS returns a short fixed sentence that names no resource, and in one case it reports a different request member than the one AWS validates first. The rule being broken is that for these refusals ECS's message is a constant independent of the request (every recording in the corpus shows the same sentence for the same condition), while the emulator's is request-derived. Recorded texts below are the exact recorded values.

THIS BUNDLE CONTAINS 7 CAUSES.

Cause 1 of 7 — RegisterContainerInstance validates the wrong member first.
AWS: Smithy: RegisterContainerInstance input {cluster, instanceIdentityDocument, instanceIdentityDocumentSignature, ...}; InvalidParameterException (client, 400). Recording: status 400 with message 'accountId cannot be blank.' — a validation message naming the blank member of the request.
Emulator: status 400 but message 'instanceIdentityDocument must be a JSON document carrying a non-empty instanceId', i.e. it complains about a different member than the one AWS reports for this request.
Why: a client (or a test) that keys on which member was rejected is pointed at the wrong field, so it 'fixes' the wrong part of the request.
Fix: the validation order and member naming in services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/register_container_instance.rs.
Traces: ecs-container-instance-lifecycle#task#2.

Cause 2 of 7 — Fargate task-level cpu refusal names both members instead of the missing one.
AWS: Smithy: TaskDefinition.cpu — 'This field is required for Fargate'. Recording: status 400, message "Fargate requires that 'cpu' be defined at the task level." — the message names the single member that is missing.
Emulator: status 400, message "Fargate requires task-level 'cpu' and 'memory' to be specified".
Why: the message no longer identifies which member the caller must supply.
Fix: services/.../src/ops/register_task_definition.rs Fargate validation messages.
Traces: ecs-fargate-awsvpc-task-definition-rendered-defaults#task#4 (+1 member, ecs-fargate-awsvpc-task-definition-rendered-defaults#task#5).

Cause 3 of 7 — StopTask not-found message embeds the task ARN and cluster name.
AWS: Smithy: StopTask errors include InvalidParameterException (client, 400). Recording: status 400, message 'The referenced task was not found'.
Emulator: status 400, message 'the referenced task "<task arn>" was not found in cluster "<cluster>"'.
Why: clients matching the documented fixed sentence do not recognise the refusal.
Fix: services/.../src/ops/stop_task.rs error construction.
Traces: ecs-run-task-and-task-protection#task#9 (+1 member, ecs-task-plane-reads-and-launch-errors#task#7).

Cause 4 of 7 — RunTask cluster-not-found message embeds the cluster name.
AWS: recording: status 400, message 'Cluster not found.'
Emulator: status 400, message 'cluster "<name>" was not found'.
Fix: services/.../src/ops/run_task.rs error construction.
Traces: ecs-run-task-and-task-protection#task#10 (+1 member, ecs-task-plane-reads-and-launch-errors#task#6).

Cause 5 of 7 — CreateService cluster-not-found message embeds the cluster name.
AWS: recording: status 400, message 'Cluster not found.'
Emulator: status 400, message 'cluster "<name>" was not found'.
Fix: services/.../src/ops/create_service.rs error construction.
Traces: ecs-service-lifecycle-on-cluster#task#9.

Cause 6 of 7 — UpdateService service-not-found message embeds the service and cluster names.
AWS: recording: status 400, message 'Service not found.'
Emulator: status 400, message 'service "<name>" was not found in cluster "<cluster>"'.
Fix: services/.../src/ops/update_service.rs error construction.
Traces: ecs-service-lifecycle-on-cluster#task#10.

Cause 7 of 7 — DescribeTaskDefinition refusal message embeds the family:revision.
AWS: recording: status 400, message 'Unable to describe task definition.'
Emulator: status 400, message 'unable to describe task definition "<family>:<revision>"'.
Fix: services/.../src/ops/describe_task_definition.rs error construction.
Traces: ecs-task-definition-lifecycle#task#9 (+1 member, ecs-task-definition-revision-lifecycle#task#7).

## Evidence
All seven causes match on HTTP status (400) and differ only in the message text / the member named:
Cause 1 (ecs-container-instance-lifecycle#task#2): $.message: value_mismatch (expected 'accountId cannot be blank.', got 'instanceIdentityDocument must be a JSON document carrying a non-empty instanceId').
Cause 2 (ecs-fargate-awsvpc-task-definition-rendered-defaults#task#4): expected "Fargate requires that 'cpu' be defined at the task level.", got "Fargate requires task-level 'cpu' and 'memory' to be specified".
Cause 3 (ecs-run-task-and-task-protection#task#9): expected 'The referenced task was not found', got 'the referenced task "arn:aws:ecs:us-west-2:000000000000:task/.../00000000-0000-0000-0000-000000000000" was not found in cluster "..."'.
Cause 4 (ecs-run-task-and-task-protection#task#10): expected 'Cluster not found.', got 'cluster "nonexistent-cluster-does-not-exist" was not found'.
Cause 5 (ecs-service-lifecycle-on-cluster#task#9): expected 'Cluster not found.', got 'cluster "sim2real-probe-missing-cluster" was not found'.
Cause 6 (ecs-service-lifecycle-on-cluster#task#10): expected 'Service not found.', got 'service "sim2real-probe-missing-service" was not found in cluster "..."'.
Cause 7 (ecs-task-definition-lifecycle#task#9): expected 'Unable to describe task definition.', got 'unable to describe task definition "sim2real-probe-ecs-taskdef-642dc70d:999"'.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus (replay_wire_corpus).
2. Steps: ecs-container-instance-lifecycle#task#2; ecs-fargate-awsvpc-task-definition-rendered-defaults#task#4; ecs-run-task-and-task-protection#task#9 and #task#10; ecs-service-lifecycle-on-cluster#task#9 and #task#10; ecs-task-definition-lifecycle#task#9.
3. By hand: RunTask{cluster:'does-not-exist'} -> 400 with an interpolated message instead of 'Cluster not found.'; UpdateService on a missing service -> 400 'service "..." was not found in cluster "..."' instead of 'Service not found.'; DescribeTaskDefinition on a missing revision -> interpolated message instead of 'Unable to describe task definition.'
