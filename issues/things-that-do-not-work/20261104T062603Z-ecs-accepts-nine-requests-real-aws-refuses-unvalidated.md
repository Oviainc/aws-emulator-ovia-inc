Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.run-task-container-override-exceeds-taskdef-limits-rejected ecs.run-task-cpu-memory-out-of-range-rejected ecs.run-task-cpu-override-exceeds-taskdef-limit-rejected ecs.run-task-overrides-unknown-container-name-rejected ecs.run-task-environment-override-injected ecs.ephemeral-storage-request-is-provisioned-inside-the-container ecs.fargate-task-definition-pidmode-host-rejected ecs.fargate-task-definition-with-awslogs-and-no-execution-role-rejected ecs.escc-windows-fargate-task-definition-rejects-a-quarter-vcpu ecs.escc-windows-service-rejects-a-linux-platform-version ecs.escc-floor-and-ceiling-both-one-hundred-rejected-on-update ecs.escc-negative-grace-period-rejected
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.run-task-container-override-exceeds-taskdef-limits-rejected|_expect_error#11|AssertionError|raise AssertionError(f'expected {codes!r}, but the call succeeded: {result!r}')
Issue-Key: ecs.run-task-cpu-memory-out-of-range-rejected|_refusal#11|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ecs.run-task-cpu-override-exceeds-taskdef-limit-rejected|_refusal#13|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ecs.run-task-overrides-unknown-container-name-rejected|_refusal#11|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ecs.run-task-environment-override-injected|_refusal#12|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ecs.ephemeral-storage-request-is-provisioned-inside-the-container|_client_error#9|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Related: 20261024T081305Z-ecs-is-overly-permissive-ten-validations-authorizations-aws.md
Issue-Related: 20261103T041326Z-ecs-createservice-runtask-createcluster-describeservices-accept-seven-request.md
Issue-Related: 20261103T041326Z-ecs-registertaskdefinition-7-fargate-firelens-validation-and-representation.md
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ECS accepts nine requests real AWS refuses: unvalidated RunTask overrides, Fargate task-definition constraints and service deployment bounds (9 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Nine independent validations are missing. In every case the emulator answers HTTP 200 with a created/launched resource where AWS answers 400 with ClientException/InvalidParameterException and no resource.

Cause 1 of 9: RunTask accepts a containerOverride memory above the task definition's container limit. AWS refuses with InvalidParameterException. Emulator: 200 with overrides {'containerOverrides': [{'name': 'app', 'memory': 4096}]} on a 512 MiB task. Fix: RunTask override validation. Trace: ecs.run-task-container-override-exceeds-taskdef-limits-rejected.

Cause 2 of 9: RunTask accepts a task-level cpu/memory override outside the Fargate cpu/memory matrix. AWS (Smithy Task.memory documents the exact permitted cpu/memory pairs, e.g. 256 cpu with 512/1024/2048 memory) refuses with InvalidParameterException. Emulator: 200 with overrides {'cpu': '256', 'memory': '999999'}. Fix: RunTask override validation. Trace: ecs.run-task-cpu-memory-out-of-range-rejected.

Cause 3 of 9: RunTask accepts a containerOverride cpu above the task's cpu budget. AWS refuses with InvalidParameterException. Emulator: 200 with containerOverrides [{'name':'app','cpu':300}] on a 256-cpu task. Fix: RunTask override validation. Trace: ecs.run-task-cpu-override-exceeds-taskdef-limit-rejected.

Cause 4 of 9: RunTask accepts containerOverrides naming a container absent from the task definition. AWS refuses with InvalidParameterException naming the unknown container. Emulator: 200, the bogus override is simply echoed back on the task. Fix: RunTask override validation (resolve each override name against containerDefinitions). Traces: ecs.run-task-overrides-unknown-container-name-rejected, ecs.run-task-environment-override-injected.

Cause 5 of 9: RegisterTaskDefinition accepts ephemeralStorage.sizeInGiB below the supported minimum. AWS: Smithy com.amazonaws.ecs#EphemeralStorage.sizeInGiB — "The minimum supported value is 21 GiB and the maximum supported value is 200 GiB" — so 20 is refused with ClientException/InvalidParameterException. Emulator: 200, the revision registers with ephemeralStorage {'sizeInGiB': 20}. Fix: RegisterTaskDefinition validation. Trace: ecs.ephemeral-storage-request-is-provisioned-inside-the-container.

Cause 6 of 9: RegisterTaskDefinition accepts pidMode=host on a Fargate-only task definition. AWS refuses it (pidMode host is not supported for Fargate). Emulator: 200. Fix: RegisterTaskDefinition Fargate-compatibility validation. Trace: ecs.fargate-task-definition-pidmode-host-rejected.

Cause 7 of 9: RegisterTaskDefinition accepts a Fargate task definition using the awslogs log driver with no executionRoleArn. AWS refuses with ClientException. Emulator: 200. Fix: RegisterTaskDefinition validation. Trace: ecs.fargate-task-definition-with-awslogs-and-no-execution-role-rejected.

Cause 8 of 9: RegisterTaskDefinition accepts a Windows Fargate task definition at 0.25 vCPU. AWS refuses it (Windows containers on Fargate have a higher minimum task size). Emulator: 200. Fix: RegisterTaskDefinition Fargate/platformFamily validation. Trace: ecs.escc-windows-fargate-task-definition-rejects-a-quarter-vcpu.

Cause 9 of 9: service deployment bounds are unvalidated — CreateService accepts a Linux platformVersion for a Windows task definition, UpdateService accepts minimumHealthyPercent=100 together with maximumPercent=100, and CreateService accepts a negative healthCheckGracePeriodSeconds. AWS refuses each with InvalidParameterException. Emulator: 200 for all three. Fix: Create/UpdateService request validation. Traces: ecs.escc-windows-service-rejects-a-linux-platform-version, ecs.escc-floor-and-ceiling-both-one-hundred-rejected-on-update, ecs.escc-negative-grace-period-rejected.

All nine: a client that relies on AWS rejecting a malformed request gets a resource it cannot use, and its negative tests pass against the emulator and fail against AWS.

## Evidence
Replayed details (one per cause):
- `AssertionError: expected ('InvalidParameterException',), but the call succeeded: {'tasks': [... 'overrides': {'containerOverrides': [{'name': 'app', 'memory': 4096}]} ...]}` (HTTPStatusCode 200)
- `AssertionError: expected a rejection, but the call SUCCEEDED and returned {... 'overrides': {'cpu': '256', 'memory': '999999'} ...}`
- `AssertionError: expected a rejection, but the call SUCCEEDED and returned {... 'containerOverrides': [{'name': 'app', 'cpu': 300}] ...}`
- `AssertionError: expected a rejection, but the call SUCCEEDED and returned {... 'containerOverrides': [{'name': 'does-not-exist', 'command': ['echo','hi']}] ...}`
- `AssertionError: real AWS refuses this call, but it SUCCEEDED and returned {'taskDefinition': {... 'ephemeralStorage': {'sizeInGiB': 20}}}`
- `AssertionError: the call succeeded; real AWS rejects it, so there is no error` (pidMode host)
- `AssertionError: the call succeeded; real AWS rejects it, so there is no error code` (Fargate awslogs without execution role)
- `AssertionError: the call succeeded; real AWS rejects it, so there is no error code` (Windows Fargate 0.25 vCPU)
- `AssertionError: the call succeeded; real AWS rejects it` (Windows service with Linux platformVersion; floor+ceiling both 100; negative grace period)
Names/ids in the quoted payloads are per-run values; the 200-instead-of-400 outcome is the requirement.

## Steps to Reproduce
For each cause issue the request described and observe HTTP 200 instead of a 400 refusal:
1. RunTask with containerOverrides memory 4096 against a 512 MiB task definition.
2. RunTask with overrides.cpu='256', overrides.memory='999999'.
3. RunTask with containerOverrides cpu 300 against a 256-cpu task.
4. RunTask with containerOverrides naming a container not in containerDefinitions.
5. RegisterTaskDefinition with ephemeralStorage.sizeInGiB=20 and requiresCompatibilities=[FARGATE].
6. RegisterTaskDefinition with pidMode='host' and requiresCompatibilities=[FARGATE].
7. RegisterTaskDefinition with a Fargate container using logDriver awslogs and no executionRoleArn.
8. RegisterTaskDefinition for a Windows platformFamily at cpu='256'.
9. CreateService for a Windows task definition with a Linux platformVersion; UpdateService with minimumHealthyPercent=100 and maximumPercent=100; CreateService with healthCheckGracePeriodSeconds=-1.
