Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__kms__logs__ssm.xsvc-ecs-3-secret-injection-cmk-disabled-rejected ecs__kms__logs__ssm.xsvc-ecs-3-secret-injection-no-kms-decrypt-rejected ecs__kms__logs__ssm.xsvc-ecs-3-secret-injection-task-role-grants-rejected
Issue-Key: ecs__kms__logs__ssm.xsvc-ecs-3-secret-injection-cmk-disabled-rejected|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: ecs__kms__logs__ssm.xsvc-ecs-3-secret-injection-no-kms-decrypt-rejected|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: ecs__kms__logs__ssm.xsvc-ecs-3-secret-injection-task-role-grants-rejected|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ecs__kms__logs__ssm: container secret injection never happens, so a disabled CMK, a missing kms:Decrypt and a task-role-only grant are all unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 1: a task whose container declares `secrets` never performs the secret resolution, so none of the KMS/IAM failure modes around it can be observed.

AWS: at task start the ECS agent resolves each container secret through the EXECUTION role (SSM Parameter Store / Secrets Manager plus kms:Decrypt on the CMK). The task fails to start — with a documented stopped reason naming the resolution failure — when the CMK is disabled, when the execution role lacks kms:Decrypt, or when only the TASK role (not the execution role) carries the grant; it starts and injects the value when the execution role is fully permitted.

Emulator: all three checks poll until the 60s budget expires. No secret resolution is attempted, so a disabled CMK, a missing kms:Decrypt and a task-role-only grant are indistinguishable from a correct configuration, and the correct configuration never injects anything either.

Why: a client that uses container secrets cannot verify its KMS key policy or its execution-role policy; each of these misconfigurations is a hard task-start failure on real AWS and a silent no-op here.

Fix: the ECS task-start secret-resolution step — resolve each container `secrets` entry via the execution role, enforce the KMS key state and the kms:Decrypt authorization, and record the task's start failure (stopCode/stoppedReason) when resolution fails.

Traces: the three failing checks in trace_ids. The open issue 20261104T044754Z-ecs-logs-secretsmanager-a-task-whose-container-secrets.md covers the Secrets Manager variant for a different suite's replay keys and does not carry these.

## Evidence
All three checks end with `Error: Check timed out after 60.0s`. Expected (AWS): each task reaches lastStatus=STOPPED with a stoppedReason naming the secret/KMS resolution failure (and the permitted control reaches RUNNING with the value injected into the container's environment).

## Steps to Reproduce
1. Create a CMK and an SSM SecureString parameter encrypted with it; RegisterTaskDefinition with a container `secrets` entry naming that parameter and an execution role.
2. Disable the CMK and RunTask -> no start failure is ever recorded.
3. Remove kms:Decrypt from the execution role and RunTask -> same.
4. Grant kms:Decrypt only to the task role and RunTask -> same; the fully-permitted control never injects the value either.
