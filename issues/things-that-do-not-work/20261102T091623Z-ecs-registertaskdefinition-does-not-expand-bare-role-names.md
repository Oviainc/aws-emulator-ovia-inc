Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__iam.ecracc-bare-role-names-expand-to-role-arns-at-register ecr__ecs__iam.ecracc-compatibilities-drop-fargate-for-an-ecr-image-without-an-execution-role ecr__ecs__iam.ecracc-fargate-ecr-image-without-an-execution-role-rejected ecr__ecs__iam.ecracc-task-role-presence-drives-the-task-iam-role-required-attribute
Issue-Key: ecr__ecs__iam.ecracc-bare-role-names-expand-to-role-arns-at-register|run#7|AssertionError|assert (read.get("executionRoleArn"), read.get("taskRoleArn")) == (exec_arn, task_arn), (
Issue-Key: ecr__ecs__iam.ecracc-compatibilities-drop-fargate-for-an-ecr-image-without-an-execution-role|run#11|AssertionError|assert "FARGATE" not in no_role["compatibilities"], (
Issue-Key: ecr__ecs__iam.ecracc-fargate-ecr-image-without-an-execution-role-rejected|_error#15|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error")
Issue-Key: ecr__ecs__iam.ecracc-task-role-presence-drives-the-task-iam-role-required-attribute|run#12|AssertionError|assert _TASK_IAM_ROLE in attrs(read_with_task), (
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# ecs: RegisterTaskDefinition does not expand bare role names, drop FARGATE, require an execution role for ECR images, or set the task-iam-role attribute

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTaskDefinition

## Description
This issue carries 4 root causes, all in RegisterTaskDefinition (Smithy: input taskRoleArn/executionRoleArn/requiresCompatibilities; errors AccessDeniedException 403, ClientException 400, InvalidParameterException 400, LimitExceededException 400, ServerException 500; output taskDefinition).

Cause 1 of 4: a bare IAM role NAME passed as taskRoleArn or executionRoleArn is not expanded to the role's ARN.
AWS: RegisterTaskDefinition accepts a short role name and stores the resolved IAM ARN, so DescribeTaskDefinition reads back 'arn:aws:iam::<account>:role/<roleName>' for both members.
Emulator: stores and returns the bare names verbatim.
Why: a client that compares the registered revision against the role ARNs it intended, or that passes the value on to IAM, gets a name where an ARN is required.
Fix: role-reference resolution in the RegisterTaskDefinition handler.
Traces: ecr__ecs__iam.ecracc-bare-role-names-expand-to-role-arns-at-register.

Cause 2 of 4: compatibilities still advertises FARGATE for a revision that cannot run there.
AWS: an image hosted in ECR cannot be pulled without an execution role, so a revision with an ECR image and no executionRoleArn comes back with FARGATE absent from taskDefinition.compatibilities (EC2 only).
Emulator: returns compatibilities ['EC2', 'FARGATE'].
Why: a deployer that reads compatibilities to choose a launch type picks Fargate for a revision that cannot start.
Fix: the compatibility derivation in RegisterTaskDefinition.
Traces: ecr__ecs__iam.ecracc-compatibilities-drop-fargate-for-an-ecr-image-without-an-execution-role.

Cause 3 of 4: requiresCompatibilities=[FARGATE] with an ECR image and no execution role is accepted.
AWS: that combination is refused — recorded behavior only for the exact text; the operation's modeled client-side class for this is ClientException (HTTP 400) and the recording shows the register call failing rather than returning a revision.
Emulator: the call SUCCEEDS and returns a task definition revision.
Why: an invalid revision is registered and only fails later at RunTask, where the cause is much harder to attribute.
Fix: the RegisterTaskDefinition validation that pairs requiresCompatibilities with the image registry and executionRoleArn.
Traces: ecr__ecs__iam.ecracc-fargate-ecr-image-without-an-execution-role-rejected.

Cause 4 of 4: requiresAttributes omits the task-IAM-role capability.
AWS: a revision that sets taskRoleArn comes back requiring the attribute named 'com.amazonaws.ecs.capability.task-iam-role' in taskDefinition.requiresAttributes; a revision without a task role does not.
Emulator: requiresAttributes is empty ([]) for a revision with a task role.
Why: placement and capability checks that read requiresAttributes cannot tell that the revision needs a task-IAM-role-capable agent.
Fix: the requiresAttributes derivation in RegisterTaskDefinition.
Traces: ecr__ecs__iam.ecracc-task-role-presence-drives-the-task-iam-role-required-attribute.

## Evidence
Cause 1: "bare names 'sim2real-ecracc-role-...'/'sim2real-ecracc-role-...' must expand to the IAM ARNs 'arn:aws:iam::000000000000:role/...'; read 'sim2real-ecracc-role-...'".
Cause 2: 'an ECR image with no execution role cannot be pulled on Fargate, so FARGATE must be dropped; got [EC2, FARGATE]'.
Cause 3: 'the call SUCCEEDED; real AWS rejects it, so there is no error'.
Cause 4: "a revision with a task role must require 'com.amazonaws.ecs.capability.task-iam-role'; got []".

## Steps to Reproduce
1. RegisterTaskDefinition passing bare role names for taskRoleArn and executionRoleArn, then DescribeTaskDefinition.
2. RegisterTaskDefinition with a container image in ECR and no executionRoleArn and read compatibilities.
3. The same with requiresCompatibilities=['FARGATE'] and observe it is accepted.
4. RegisterTaskDefinition with taskRoleArn set and read requiresAttributes.
5. Or replay the check ids in trace_ids.
