Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__logs.ecs-task-awslogs-stream-name-and-stdout ecs__logs.log-workload-producers-awslogs-one-event-per-line-verbatim ecs__logs.log-workload-producers-awslogs-long-line-split-at-event-size-limit ecs__logs.log-workload-producers-awslogs-create-group-denied-without-create-log-group ecs__logs.awslogs-without-create-log-stream-delivers-nothing ecs__logs.awslogs-exec-role-scoped-to-one-group-is-denied-on-another ecs__logs.log-workload-producers-failed-start-delivers-no-log-events-unlike-an-unlogged-run ecs__logs.deep-ecs-guest-identity-awsvpc-task-reachable-on-its-eni-ip-only-while-its-sg-allows-it ecs__logs.deep-ecs-guest-identity-task-role-is-the-guests-credential-source-and-identity ecs__logs.ecs-readonly-mount-point-write-rejected-while-the-writer-sibling-still-writes ecs__logs.ecs-runtime-platform-arm64-task-reports-aarch64-to-its-own-process ecs__logs.ecs-service-connect-client-task-reaches-the-server-service-by-its-discovery-name ecs__logs.svcreplace-stopped-standalone-task-is-never-relaunched-while-a-service-managed-one-is-replaced
Issue-Key: ecs__logs.ecs-task-awslogs-stream-name-and-stdout|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: ecs__logs.log-workload-producers-awslogs-one-event-per-line-verbatim|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: ecs__logs.log-workload-producers-awslogs-long-line-split-at-event-size-limit|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: ecs__logs.log-workload-producers-awslogs-create-group-denied-without-create-log-group|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: ecs__logs.awslogs-without-create-log-stream-delivers-nothing|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: ecs__logs.awslogs-exec-role-scoped-to-one-group-is-denied-on-another|_poll#7|Timeout|last = probe()
Issue-Key: ecs__logs.log-workload-producers-failed-start-delivers-no-log-events-unlike-an-unlogged-run|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: ecs__logs.deep-ecs-guest-identity-awsvpc-task-reachable-on-its-eni-ip-only-while-its-sg-allows-it|_body#3|KeyError|srv_ip = _eni_details(srv_task)["privateIPv4Address"]
Issue-Key: ecs__logs.deep-ecs-guest-identity-task-role-is-the-guests-credential-source-and-identity|_poll#3|Timeout|time.sleep(interval_s)
Issue-Key: ecs__logs.ecs-readonly-mount-point-write-rejected-while-the-writer-sibling-still-writes|_poll_until#7|Timeout|last = probe()
Issue-Key: ecs__logs.ecs-runtime-platform-arm64-task-reports-aarch64-to-its-own-process|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: ecs__logs.ecs-service-connect-client-task-reaches-the-server-service-by-its-discovery-name|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: ecs__logs.svcreplace-stopped-standalone-task-is-never-relaunched-while-a-service-managed-one-is-replaced|_drop#4|Timeout|if _safe(lambda: ec2.delete_vpc(VpcId=vpc_id)) is not None:
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ecs__logs: the awslogs driver never delivers, awsvpc tasks have no private IP, and no in-guest behaviour (task-role identity, read-only mounts, arm64 platform, Service Connect, task replacement) is observable (7 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 7: the awslogs log driver never creates a stream or delivers events.
AWS: each container's stdout is written to the configured group as one log event per line, verbatim; a line longer than the event size limit is split; the stream is named from awslogs-stream-prefix plus the container name and task id; a task whose execution role lacks logs:CreateLogStream (or logs:CreateLogGroup with awslogs-create-group) fails delivery; an execution role scoped to one group is denied on another; a task that fails to start delivers no events.
Emulator: every one of these checks polls until the 60s budget expires — nothing is ever written, so neither the positive deliveries nor the authorization denials can be observed.
Fix: the ECS awslogs log-driver component (stream naming, per-line event emission, size-limit splitting) and the CloudWatch Logs authorization it must honour.
Traces: ecs__logs.ecs-task-awslogs-stream-name-and-stdout, ecs__logs.log-workload-producers-awslogs-one-event-per-line-verbatim, ecs__logs.log-workload-producers-awslogs-long-line-split-at-event-size-limit, ecs__logs.log-workload-producers-awslogs-create-group-denied-without-create-log-group, ecs__logs.awslogs-without-create-log-stream-delivers-nothing, ecs__logs.awslogs-exec-role-scoped-to-one-group-is-denied-on-another, ecs__logs.log-workload-producers-failed-start-delivers-no-log-events-unlike-an-unlogged-run.

Cause 2 of 7: an awsvpc task's attachment carries no privateIPv4Address, so the task is unreachable on its own ENI.
AWS: Attachment.details for an ElasticNetworkInterface includes the network interface ID, MAC address, subnet ID and private IPv4 address (Smithy com.amazonaws.ecs#Attachment).
Emulator: the key is absent — `KeyError: 'privateIPv4Address'` — so the security-group reachability scenario cannot even address the task.
Fix: the ECS awsvpc ENI materialisation.
Trace: ecs__logs.deep-ecs-guest-identity-awsvpc-task-reachable-on-its-eni-ip-only-while-its-sg-allows-it.

Cause 3 of 7: the task role is not the guest's credential source/identity.
AWS: the container's credential provider serves the task role, so an STS GetCallerIdentity from inside the task reports the assumed-role identity derived from taskRoleArn.
Emulator: the check polls until the budget expires; nothing in the guest ever runs or has credentials.
Fix: the ECS task runtime plus the task credentials endpoint.
Trace: ecs__logs.deep-ecs-guest-identity-task-role-is-the-guests-credential-source-and-identity.

Cause 4 of 7: a readOnly mountPoint is not enforced inside the container.
AWS: a write into a read-only mount fails with a read-only-filesystem error while the writer sibling's write succeeds.
Emulator: polls to the budget; neither side is observable.
Fix: the ECS task runtime's volume mounting.
Trace: ecs__logs.ecs-readonly-mount-point-write-rejected-while-the-writer-sibling-still-writes.

Cause 5 of 7: runtimePlatform cpuArchitecture is not reflected to the process.
AWS: an ARM64 task's process sees aarch64.
Emulator: polls to the budget.
Fix: the ECS task runtime's platform selection.
Trace: ecs__logs.ecs-runtime-platform-arm64-task-reports-aarch64-to-its-own-process.

Cause 6 of 7: ECS Service Connect provides no discovery.
AWS: a client task reaches the server service by its Service Connect discovery name (via the injected proxy/alias).
Emulator: polls to the budget; no Service Connect attachment or alias is ever usable.
Fix: the ECS Service Connect attachment/proxy wiring (the Attachment type 'Service Connect' with portName/clientAliases/discoveryName the model documents).
Trace: ecs__logs.ecs-service-connect-client-task-reaches-the-server-service-by-its-discovery-name.

Cause 7 of 7: a stopped service-managed task is not replaced (and a standalone one must not be).
AWS: stopping a task that belongs to a service makes the scheduler launch a replacement; a stopped standalone RunTask task is never relaunched.
Emulator: polls to the budget — no replacement ever appears, so the contrast cannot be observed.
Fix: the ECS service scheduler's replacement loop.
Trace: ecs__logs.svcreplace-stopped-standalone-task-is-never-relaunched-while-a-service-managed-one-is-replaced.

## Evidence
Cause 2: `KeyError: 'privateIPv4Address'` while reading the task attachment details. All other listed checks end with `Error: Check timed out after 60.0s`. Expected (AWS) per cause: a log stream named <prefix>/<container>/<task-id> containing one event per stdout line; attachments carrying privateIPv4Address; GetCallerIdentity inside the guest reporting the task role's assumed-role ARN; a read-only mount write failing; uname reporting aarch64; the client task resolving the Service Connect discovery name; a replacement task appearing for the service-managed task only.

## Steps to Reproduce
1. RunTask with an awslogs logConfiguration and a container that prints known lines; poll DescribeLogStreams/GetLogEvents -> nothing. Repeat with an execution role missing logs:CreateLogStream / scoped to another group -> no denial observable.
2. DescribeTasks an awsvpc task and read attachments[0].details['privateIPv4Address'] -> KeyError.
3. RunTask with a taskRoleArn and call STS from the container -> no identity.
4. Mount a volume readOnly in one container and read-write in a sibling and attempt writes -> nothing observable.
5. RegisterTaskDefinition with runtimePlatform cpuArchitecture=ARM64 and report the arch from inside -> nothing.
6. Create two services with serviceConnectConfiguration and have the client reach the server's discovery name -> never resolves.
7. StopTask on a service-managed task and on a standalone task -> no replacement for either.
