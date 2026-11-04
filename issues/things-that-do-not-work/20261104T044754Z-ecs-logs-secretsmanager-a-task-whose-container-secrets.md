Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__logs__secretsmanager.ecs-secret-injection-needs-execution-role-not-task-role ecs__logs__secretsmanager.ecs-secret-json-key-selector-injects-only-that-key
Issue-Key: ecs__logs__secretsmanager.ecs-secret-injection-needs-execution-role-not-task-role|_poll_until#17|Timeout|last = probe()
Issue-Key: ecs__logs__secretsmanager.ecs-secret-json-key-selector-injects-only-that-key|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Commit: 7edae7e43dd2ef932ae2dbfd8dbf1e7a775f7569

# ecs__logs__secretsmanager: a task whose container `secrets` valueFrom names a Secrets Manager secret never reaches a terminal lifecycle and writes no awslogs stream, so both secret-injection checks hang to the harness cap

> _Note: this issue was identified against the code base at commit `7edae7e43dd2ef932ae2dbfd8dbf1e7a775f7569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
One cause.

Cause 1 of 1: a task launched from a task definition whose container carries `secrets` entries (valueFrom = a Secrets Manager secret ARN, optionally with a `:SecretString:<json-key>` suffix) never settles: neither check ever observes a terminal task lifecycle, a log stream, or the ResourceInitializationError the negative control needs, so both run to the 60 s harness cap with no verdict.

AWS: Smithy AmazonEC2ContainerServiceV20141113 models ContainerDefinition$secrets as a list of Secret {name (required), valueFrom (required) — "The secret to expose to the container. The supported values are either the full ARN of the Secrets Manager secret or the full ARN of the parameter in the SSM Parameter Store"}. The agent resolves each valueFrom with the task definition's EXECUTION role before the container exists: when that role may not read the secret the task stops at start with Task$stopCode/Task$stoppedReason reporting a ResourceInitializationError that names the secret, and the container never starts, so the awslogs driver creates NO log stream in the configured log group; when the execution role carries the grant the task runs, the variable is injected (the whole SecretString for a bare ARN, only that key's value for a `:SecretString:<key>` suffix), the container prints it and an awslogs stream appears under the configured stream prefix. A json key the secret does not contain also stops the task at start. Either way the task reaches a terminal lastStatus (STOPPED) in bounded time.

Emulator: neither outcome is ever reached. Both checks end with "Check timed out after 60.0s" — no task lifecycle progression and no log stream is observed within the budget, so neither the positive injection nor the ResourceInitializationError rejection can be graded. Hypothesis: RunTask records a task that never advances past its initial lastStatus and the awslogs side-effect is never produced; the open issue 20261103T150504Z-ecs-task-records-omit-platform-and-sizing-fields.md reports the same never-stopping lifecycle for plain ECS tasks, but it does not carry these two checks or the secret-injection/log-stream consequence.

Why: a client cannot use ECS secret injection at all against this emulator — a task never becomes observably RUNNING or STOPPED, the injected value never appears in logs, and the execution-role-versus-task-role distinction (the whole point of the negative control) is invisible, so a task definition that AWS fails at start looks indistinguishable from one that works.

Fix: the ECS task lifecycle in the RunTask/DescribeTasks handlers and the task state module — a task must advance to a terminal lastStatus with stopCode/stoppedReason, and the container `secrets` resolution must run against the execution role and emit the awslogs stream for the configured log group/stream prefix.

Traces: ecs__logs__secretsmanager.ecs-secret-injection-needs-execution-role-not-task-role, ecs__logs__secretsmanager.ecs-secret-json-key-selector-injects-only-that-key.

## Evidence
Both failures are reported by the behavioral replay (suite simparity-behavioral-aws-v1, service group ecs__logs__secretsmanager 0/3):

[mismatch] ecs__logs__secretsmanager.ecs-secret-injection-needs-execution-role-not-task-role
Error: Check timed out after 60.0s
(check doc: "A secret in valueFrom is fetched with the execution role before the container exists: the grant on the task role fails the task at start with ResourceInitializationError and no stream, and the same statement on the execution role lets the task print the value." The check asserts the fresh log group has no streams, then runs the task.)

[mismatch] ecs__logs__secretsmanager.ecs-secret-json-key-selector-injects-only-that-key
Error: Check timed out after 60.0s
(check doc: "A `secrets` valueFrom with a json-key suffix injects ONLY that key's value while the bare ARN injects the whole JSON document; a json-key the secret lacks stops the task at start." The check allows _TASK_TIMEOUT_S = 600 s for the task and _LOG_TIMEOUT_S = 300 s for the log stream, so it hangs because nothing it polls ever changes.)

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. secretsmanager: CreateSecret with a JSON SecretString such as {"USERNAME":"u","PASSWORD":"p"}.
3. logs: CreateLogGroup; confirm DescribeLogStreams is empty.
4. ecs: CreateCluster, then RegisterTaskDefinition with a container whose `secrets` list has {name: "PASSWORD", valueFrom: "<secret-arn>:SecretString:PASSWORD"} and an awslogs logConfiguration pointing at that log group, an execution role that may read the secret, and a command that echoes the variable.
5. RunTask, then poll DescribeTasks for a terminal lastStatus and logs:DescribeLogStreams/GetLogEvents for the printed value -> expect a STOPPED task and a stream carrying only that key's value; observed: the task never reaches a terminal lastStatus and no stream is ever created.
6. Repeat with the secret grant moved to the TASK role only -> expect the task to stop at start with a ResourceInitializationError naming the secret and no log stream.
7. Replay the two checks in trace_ids.
