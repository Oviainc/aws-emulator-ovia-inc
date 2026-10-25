Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-container-instance-and-attribute-errors
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-container-instance-and-attribute-errors ecs-err-run-task-fargate-on-ec2-task-definition ecs-run-task-and-task-protection ecs-task-plane-reads-and-launch-errors ecs-fargate-awsvpc-task-definition-rendered-defaults ecs-cluster-describe-and-list-lifecycle
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|RegisterContainerInstance|4xx->2xx|$.__type
Issue-Key: ecs|RegisterContainerInstance|4xx->2xx|$.message
Issue-Key: ecs|RegisterContainerInstance|4xx->2xx|status
Issue-Key: ecs|RunTask|4xx->2xx|$.__type
Issue-Key: ecs|RunTask|4xx->2xx|$.message
Issue-Key: ecs|RunTask|4xx->2xx|status
Issue-Key: ecs|ListTasks|2xx->2xx|$.taskArns[*]
Issue-Key: ecs|RegisterTaskDefinition|4xx->2xx|$.__type
Issue-Key: ecs|RegisterTaskDefinition|4xx->2xx|$.message
Issue-Key: ecs|RegisterTaskDefinition|4xx->2xx|status
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.failures[arn="arn
Issue-Key: ecs|DescribeContainerInstances|2xx->2xx|$.failures[arn="arn
Issue-Key: ecs|DescribeClusters|4xx->4xx|$.__type
Issue-Key: ecs|DescribeClusters|4xx->4xx|$.message
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# ECS accepts invalid requests with 200 and misreports describe failures

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterContainerInstance

## Description
Several ECS request-validation rules are missing entirely: the emulator answers HTTP 200 and mutates state where AWS refuses with a modeled 400 client error. A related pair of defects is in the batch-describe failure channel, which either drops the failure entry for an unknown identifier or rewrites the identifier it echoes, and one validation path raises the wrong error type.

THIS BUNDLE CONTAINS 7 CAUSES.

Cause 1 of 7 — RegisterContainerInstance accepts an invalid instance identity document.
AWS: Smithy: RegisterContainerInstance errors include InvalidParameterException (client, HTTP 400). Recording: status 400, __type 'InvalidParameterException', message 'The identity document and identity document signature were not valid.'
Emulator: answers 200 and registers the instance; no __type, no message.
Why: unsigned/forged registrations succeed, and a client testing its error handling for a bad identity document never sees the refusal — worse, the bogus instance is then counted against the cluster.
Fix: the identity-document/signature validation in services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/register_container_instance.rs.
Traces: ecs-container-instance-and-attribute-errors#task#12.

Cause 2 of 7 — RunTask does not validate launchType against the task definition's compatibilities.
AWS: Smithy: RunTask errors include InvalidParameterException (client, HTTP 400). Recording: status 400, __type 'InvalidParameterException', message 'Task definition does not support launch_type FARGATE.'
Emulator: answers 200 and launches the task; no __type, no message.
Why: a client that expects the launch-type incompatibility to be rejected instead gets a running task that could never exist on AWS.
Fix: the launch-type/compatibility precondition in services/.../src/ops/run_task.rs.
Traces: ecs-err-run-task-fargate-on-ec2-task-definition#task#5 (+2 members, e.g. ecs-task-plane-reads-and-launch-errors#task#5).

Cause 3 of 7 — ListTasks then reports the task that should never have been created.
AWS: recording: taskArns=[] after the refused RunTask.
Emulator: taskArns carries one task ARN (status 200 both sides).
Why: the unvalidated RunTask leaves phantom state, so subsequent list/describe/stop calls operate on a task AWS would not have.
Fix: follows from Cause 2 (services/.../src/ops/run_task.rs must not persist the task); verify via services/.../src/ops/list_tasks.rs.
Traces: ecs-err-run-task-fargate-on-ec2-task-definition#task#6 (+1 member, ecs-run-task-and-task-protection#task#5).

Cause 4 of 7 — RegisterTaskDefinition accepts an invalid Fargate cpu/memory combination.
AWS: Smithy: RegisterTaskDefinition errors include ClientException (client, HTTP 400); TaskDefinition.memory documents the legal cpu/memory pairs (256 CPU supports 512/1024/2048 MiB only). Recording: status 400, __type 'ClientException', message 'No Fargate configuration exists for given values: 256 CPU, 4096 memory. See the Amazon ECS documentation for the valid values.'
Emulator: answers 200 and registers the definition; no __type, no message.
Why: a client gets a Fargate task definition that cannot be launched on AWS, and the failure surfaces much later.
Fix: the Fargate cpu/memory pair table in services/.../src/ops/register_task_definition.rs validation.
Traces: ecs-fargate-awsvpc-task-definition-rendered-defaults#task#6 (+1 member, ecs-fargate-awsvpc-task-definition-rendered-defaults#task#7).

Cause 5 of 7 — DescribeTasks omits the failure entry for an unknown task ARN.
AWS: Smithy: DescribeTasks -> output {tasks, failures : Failures}. Recording: failures=[{arn: <requested task arn>, reason: 'MISSING'}].
Emulator: returns no failure entry at all for that ARN (status 200).
Why: a client that reconciles requested identifiers against tasks+failures finds an identifier accounted for in neither list and cannot tell whether the task is missing or the call was partial.
Fix: the failure channel in services/.../src/ops/describe_tasks.rs.
Traces: ecs-task-plane-reads-and-launch-errors#task#4.

Cause 6 of 7 — DescribeContainerInstances rewrites the ARN it echoes in failures.
AWS: recording: failures[0].arn is the ARN exactly as requested ('arn:aws:ecs:<region>:<account>:container-instance/<uuid>'), reason 'MISSING'.
Emulator: echoes a different ARN, with the cluster name inserted into the resource path ('.../container-instance/<cluster>/<uuid>').
Why: a client that matches failure entries back to the identifiers it sent finds no match, so the MISSING report is unattributable.
Fix: the failure construction in services/.../src/ops/describe_container_instances.rs — echo the caller's identifier verbatim.
Traces: ecs-container-instance-and-attribute-errors#task#4.

Cause 7 of 7 — DescribeClusters raises the wrong error type for an invalid include value.
AWS: Smithy: DescribeClusters errors are AccessDeniedException, ClientException, InvalidParameterException and ServerException — ValidationException is not a modeled error for this service. Recording: status 400, __type 'InvalidParameterException', message 'The cluster identifier is invalid'.
Emulator: status 400 but __type 'ValidationException' with a generic enum-constraint message.
Why: a client's error-type dispatch (catching InvalidParameterException) does not match an unmodeled ValidationException, so the refusal is handled as an unknown error.
Fix: the include-field validation error in services/.../src/ops/describe_clusters.rs must raise the modeled InvalidParameterException.
Traces: ecs-cluster-describe-and-list-lifecycle#task#6.

## Evidence
Cause 1 (ecs-container-instance-and-attribute-errors#task#12, RegisterContainerInstance): status: expected 400, got 200; $.__type missing (expected 'InvalidParameterException'); $.message missing (expected 'The identity document and identity document signature were not valid.').
Cause 2 (ecs-err-run-task-fargate-on-ec2-task-definition#task#5, RunTask): status expected 400, got 200; $.__type missing (expected 'InvalidParameterException'); $.message missing (expected 'Task definition does not support launch_type FARGATE.').
Cause 3 (ecs-err-run-task-fargate-on-ec2-task-definition#task#6, ListTasks): $.taskArns[0]: extra (expected None, got 'arn:aws:ecs:us-west-2:000000000000:task/<cluster>/00000000000000000000000000000001').
Cause 4 (ecs-fargate-awsvpc-task-definition-rendered-defaults#task#6, RegisterTaskDefinition): status expected 400, got 200; $.__type missing (expected 'ClientException'); $.message missing (expected 'No Fargate configuration exists for given values: 256 CPU, 4096 memory. ...').
Cause 5 (ecs-task-plane-reads-and-launch-errors#task#4, DescribeTasks): $.failures[arn="arn:aws:ecs:us-west-2:000000000000:task/11111111222233334444555566667777"]: missing (expected {'arn': ..., 'reason': 'MISSING'}, got None).
Cause 6 (ecs-container-instance-and-attribute-errors#task#4, DescribeContainerInstances): expected failure arn '.../container-instance/<uuid>' missing; extra failure arn '.../container-instance/<cluster>/<uuid>'.
Cause 7 (ecs-cluster-describe-and-list-lifecycle#task#6, DescribeClusters, 400/400): $.__type: value_mismatch (expected 'InvalidParameterException', got 'ValidationException').

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus (replay_wire_corpus).
2. Fixtures/steps: ecs-container-instance-and-attribute-errors#task#12 and #task#4; ecs-err-run-task-fargate-on-ec2-task-definition#task#5 and #task#6; ecs-fargate-awsvpc-task-definition-rendered-defaults#task#6; ecs-task-plane-reads-and-launch-errors#task#4; ecs-cluster-describe-and-list-lifecycle#task#6.
3. By hand: RegisterContainerInstance with a junk instanceIdentityDocument -> 200 instead of 400. RunTask{launchType: FARGATE} against an EC2-only task definition -> 200 and a phantom task in ListTasks. RegisterTaskDefinition{requiresCompatibilities:[FARGATE], cpu:'256', memory:'4096'} -> 200 instead of 400 ClientException. DescribeTasks with an unknown task ARN -> no failure entry. DescribeClusters{include:['NOT_A_VALID_INCLUDE_FIELD']} -> ValidationException instead of InvalidParameterException.
