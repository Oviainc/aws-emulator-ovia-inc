Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__iam__s3.fargate-task-role-missing-s3-putobject-grant-is-denied-then-repaired
Issue-Key: ecs__iam__s3.fargate-task-role-missing-s3-putobject-grant-is-denied-then-repaired|_wait_stopped#22|Timeout|time.sleep(5)
Issue-Commit: 7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae

# ecs__iam__s3: a Fargate task's container never executes, so a task-role S3 denial and its repair are never observable (1 cause)

> _Note: this issue was identified against the code base at commit `7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 1: a Fargate task launched with an awslogs-configured container never executes that container and never settles, so neither an IAM-denied s3:PutObject from the task role nor the repaired (allowed) write is ever observable.
AWS: recorded behavior only. A RunTask'd Fargate task whose container image is an AWS CLI image with a shell command runs that command inside the container; the container's STDOUT is delivered to the configured awslogs log group/stream, the task then settles to lastStatus STOPPED, and the in-container aws s3api put-object call is evaluated against the task role: with no s3:PutObject statement on the role the call fails with S3 AccessDenied and no object exists under the key, and after the single Allow statement (Action s3:PutObject, Resource arn:aws:s3:::<bucket>/*) is added to the SAME role, a task of the next revision writes the object successfully. The two stable outcome lines the container prints ("RESULT:OK key=<key>" / "RESULT:DENIED key=<key> error=<err>") carry only values taken from the request and state; the requirement is that the denial/repair distinction is observable at all.
Emulator: the scenario never reaches a verdict — the check is killed by the harness at 60s. No container command runs, no awslogs events are delivered for the container, and the task does not reach a terminal lifecycle, so both the denied attempt and the repaired write are unobservable.
Why: a client cannot use the emulator to validate an ECS task-role permission boundary at all: a task that should be denied and a task that should succeed are indistinguishable, and nothing ever finishes.
Fix: the ECS data plane that executes a task's containerDefinitions (command/image), the awslogs delivery path for container STDOUT, and the task lifecycle progression to STOPPED, plus evaluation of the in-container AWS calls against the task role (the ecs service crate's task runtime/state and its log-driver delivery).
Traces: ecs__iam__s3.fargate-task-role-missing-s3-putobject-grant-is-denied-then-repaired

Note for the fixer: the open issue 20261104T055008Z-ecs-iam-s3-a-fargate-task-never-executes.md describes a similar symptom but the replay report did not mark this check as owned by it, so this check's replay key is filed here; if that issue is the same root cause, close them together and re-replay the check id below.

## Evidence
Expected: the task's container runs its shell script, its STDOUT lands in the configured awslogs stream, the task reaches lastStatus STOPPED, and the printed outcome line is "RESULT:DENIED key=<key> error=<AccessDenied text>" for the role without the s3:PutObject statement and "RESULT:OK key=<key>" for the repaired role, with S3 HeadObject/GetObject confirming the object absent then present.
Actual: "Check timed out after 60.0s" — the harness is killed while waiting on the task/log-delivery poll loop. No container output is produced, the task never settles, and neither the denied nor the repaired write is observable.

## Steps to Reproduce
1. Replay the behavioural suite and re-run ecs__iam__s3.fargate-task-role-missing-s3-putobject-grant-is-denied-then-repaired.
2. Directly: create an S3 bucket; create an execution role and a task role, both trusted to ecs-tasks.amazonaws.com, the task role WITHOUT any s3 statement; create a CloudWatch Logs log group; RegisterTaskDefinition (FARGATE, awsvpc, container image public.ecr.aws/aws-cli/aws-cli, command = a shell script that runs `aws s3api put-object --bucket $BUCKET --key $KEY --body ...` and prints RESULT:OK/RESULT:DENIED, logConfiguration awslogs -> that log group, taskRoleArn = the task role).
3. RunTask it on a Fargate cluster and poll DescribeTasks for lastStatus STOPPED and GetLogEvents on the container's log stream for the RESULT line; observe neither ever appears.
4. Add {"Effect":"Allow","Action":["s3:PutObject"],"Resource":["arn:aws:s3:::<bucket>/*"]} to the same task role, register the next revision, RunTask again, and observe that the OK outcome is equally unobservable.
