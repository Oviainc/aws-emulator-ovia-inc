Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.ecsnt-create-service-with-a-negative-desired-count-rejected
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.ecsnt-create-service-with-a-negative-desired-count-rejected|run#8|AssertionError|assert msg == want, f"desiredCount -1 refusal message: expected {want!r}, got {msg!r}"
Issue-Commit: 6596661b7bed7b2c68eacfc83c0e7a703969540b

# ecs: CreateService refuses desiredCount -1 with RunTask's wording ("Task count can not be lesser than 0") instead of "DesiredCount can not be lesser than 0"

> _Note: this issue was identified against the code base at commit `6596661b7bed7b2c68eacfc83c0e7a703969540b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
One net-new root cause under the suite heading `ecs`.

Cause 1 of 1: CreateService's negative-desiredCount refusal carries the wrong message text — it reuses the RunTask `count` wording for the `desiredCount` member. Failing check id: ecs.ecsnt-create-service-with-a-negative-desired-count-rejected.
  AWS: Smithy: CreateService.desiredCount targets BoxedInteger with no modeled @range, and CreateService declares InvalidParameterException (errorKind client, httpError 400). The recording shows AWS refusing CreateService with desiredCount below 0 as HTTP 400, error code `InvalidParameterException`, message exactly `DesiredCount can not be lesser than 0` — the whole sentence is fixed text naming the rejected member (`DesiredCount`), with no value interpolated from the request; no service is created. RunTask's analogous refusal for its `count` member is a DIFFERENT fixed sentence (`Task count can not be lesser than 0`), because it names a different member.
  Emulator: the status (400) and the code (`InvalidParameterException`) are right, but the message is `Task count can not be lesser than 0` — RunTask's sentence, naming `Task count` instead of `DesiredCount`. Replay output: `AssertionError: desiredCount -1 refusal message: expected 'DesiredCount can not be lesser than 0', got 'Task count can not be lesser than 0'`.
  Why: the message is the only part of the refusal that identifies WHICH request member was invalid. A client (or a CLI user, or an IaC error-mapping layer) that matches on AWS's wording to attribute the failure to `desiredCount`, or that surfaces the message to an operator, is told the task *count* was wrong — a member CreateService does not even accept — so the diagnosis points at the wrong input and operators chase a non-existent field.
  Fix: the desiredCount validation in the CreateService handler of the ECS service crate — it must emit CreateService's own InvalidParameterException sentence rather than share RunTask's count-validation message. The accompanying positive assertions in the same check (the refusal leaves no service; desiredCount 0 is accepted and reads back as `desiredCount`/`runningCount` == 0) pass, so only the message text is wrong.
  Traces: ecs.ecsnt-create-service-with-a-negative-desired-count-rejected.

## Evidence
Cause 1 — expected: HTTP 400, `InvalidParameterException`, message `DesiredCount can not be lesser than 0`, and `ListServices` empty afterwards. Actual: HTTP 400 and `InvalidParameterException` (both correct) with message `Task count can not be lesser than 0`. Replay detail: `File "check_ecs_ecsnt_create_service_with_a_negative_desired_count_rejected.py", line 35, in run / assert msg == want / AssertionError: desiredCount -1 refusal message: expected 'DesiredCount can not be lesser than 0', got 'Task count can not be lesser than 0'`.

## Steps to Reproduce
1. `ecs create_cluster(clusterName=<c>)` and `ecs register_task_definition(...)` for an EC2/bridge definition.
2. Call `ecs.create_service(cluster=<c>, serviceName=<n>, taskDefinition=<td>, desiredCount=-1, launchType="EC2")`.
3. Read `Error.Code` and `Error.Message` from the ClientError: the code is `InvalidParameterException` but the message is `Task count can not be lesser than 0` instead of `DesiredCount can not be lesser than 0`.
Replay check id: ecs.ecsnt-create-service-with-a-negative-desired-count-rejected.
