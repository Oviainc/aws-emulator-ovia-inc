Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.task-state-change-sequencing ecs.update-container-instance-invalid-status-fails ecs.create-service-mismatched-launch-type-capacity-provider-rejected ecs.e2e-disassociate-capacity-provider-in-use-rejected ecs.run-task-capacity-provider-strategy-conflict-rejected ecs.capprov-zero-capacity-provider-rejects-task-and-no-container-runs
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.task-state-change-sequencing|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: ecs.update-container-instance-invalid-status-fails|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: ecs.create-service-mismatched-launch-type-capacity-provider-rejected|run#13|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ecs|PutClusterCapacityProviders|ResourceInUseException
Issue-Key: ecs.run-task-capacity-provider-strategy-conflict-rejected|run#29|AssertionError|assert placed.get("launchType") == "FARGATE", (
Issue-Key: ecs.capprov-zero-capacity-provider-rejects-task-and-no-container-runs|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Commit: 37c80e55e89881237cfb2c32173143a5950fe67d

# ecs: 5 net-new causes — EC2 hosts never register as container instances, CreateService's launchType/capacityProviderStrategy conflict message is wrong, PutClusterCapacityProviders over-refuses ResourceInUseException for a standalone task, a provider-placed task reports no launchType, and a zero-capacity provider RunTask never converges

> _Note: this issue was identified against the code base at commit `37c80e55e89881237cfb2c32173143a5950fe67d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Five independent root causes in the ECS surface. Every one is grounded in a failing check the suite report lists under `### ecs` that is marked NEITHER "ALREADY FILED" nor "OWNED by" any open issue. All five must be closed before submitting.

Cause 1 of 5: an EC2 instance launched with an ECS-capable instance profile and the standard `ECS_CLUSTER` user data never appears as a container instance of that cluster, so every body whose first step is that registration burns its whole budget without reaching a verdict.
  AWS: RunInstances with an ECS-optimized AMI (read from SSM `/aws/service/ecs/optimized-ami/amazon-linux-2023/recommended/image_id`), an instance profile carrying AmazonEC2ContainerServiceforEC2Role, and user data `echo ECS_CLUSTER=<cluster> >> /etc/ecs/ecs.config` makes the on-host ECS agent call RegisterContainerInstance; ListContainerInstances then returns that instance's ARN and DescribeContainerInstances reports it with `ec2InstanceId` equal to the launched instance id, `status: ACTIVE` and `agentConnected: true`. Smithy (com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113): UpdateContainerInstancesState input {cluster, containerInstances (required), status (required, ContainerInstanceStatus enum = ACTIVE|DRAINING|REGISTERING|DEREGISTERING|REGISTRATION_FAILED)} -> output {containerInstances, failures}, errors AccessDeniedException 403, ClientException/ClusterNotFoundException/InvalidParameterException 400, ServerException 500 — the operation is defined only over an already-registered instance, as are SubmitTaskStateChange's agent-side ordering rules. The model states the shapes, not the timing; the timing is recorded behavior only — in the recording an instance becomes a container instance well inside each check's budget.
  Emulator: the instance never registers. Both bodies poll `ecs.list_container_instances(cluster=...)` / `describe_container_instances` for a container instance whose `ec2InstanceId` is the launched id (`description="EC2 instance registers as an ECS container instance"`, timeout 200s) and are cut by the harness first: `Error: Check timed out after 180.0s`. No error and no success is ever produced for the registration.
  Why: the ECS-on-EC2 plane stays unfalsifiable for a client. Nothing scoped to a container instance can be exercised or refused — here specifically UpdateContainerInstancesState's acceptance of DRAINING and its refusal of the reportable-but-unsettable `REGISTERING` target status (AWS: HTTP 400, code `InvalidParameterException`, message `Container instances status should be one of [ACTIVE,DRAINING]`, whose whole text is fixed, with the instance left at DRAINING), and the agent-side rule that a stale SubmitTaskStateChange(status=RUNNING) must not regress a STOPPED task's `lastStatus` while the same call against an unknown cluster fails `ClientException` / `Cluster not found.` (fixed text).
  Fix: whatever stands an emulator-launched EC2 instance up as an ECS agent host, plus the ECS `register_container_instance` handler and the container-instance records in `crate::state` that ListContainerInstances/DescribeContainerInstances are served from.
  Related open issue, which does NOT own these two checks (the report marks neither of them OWNED, because their recorded replay keys no longer match the frame they now fail at): 20261109T013920Z-ecs-4-net-new-causes-ec2-launched-hosts.md, whose cause 1 is the same registration gap. Closing it without replaying the two ids below leaves them unrun.
  Traces: ecs.task-state-change-sequencing, ecs.update-container-instance-invalid-status-fails.

Cause 2 of 5: CreateService refuses a request that carries BOTH `launchType` and `capacityProviderStrategy` with the right code but the wrong message.
  AWS: CreateService rejects that combination with HTTP 400 and error code `InvalidParameterException` (Smithy: CreateService input has both `launchType` (LaunchType enum) and `capacityProviderStrategy` (CapacityProviderStrategy list); modeled errors include InvalidParameterException at httpError 400; InvalidParameterException is documented "The specified parameter isn't valid. Review the available parameters for the API request."), and no service object is created — DescribeServices for that name returns `services: []` with a `MISSING` failure. Recorded behavior for the message: the whole message is AWS's fixed sentence `Specifying both a launch type and capacity provider strategy is not supported. Remove one and try again.` — it contains no value from the request.
  Emulator: the code is right but the message is a different sentence — `You may choose a capacity provider or a launch type but not both.`
  Why: the refusal message is part of the requirement. A client (or a CLI user, or a test asserting on the diagnostic) that matches AWS's sentence does not recognise this refusal, and the emulator's wording names neither remedy AWS names.
  Fix: the CreateService handler's launchType/capacityProviderStrategy conflict validation — emit AWS's fixed sentence verbatim with InvalidParameterException.
  Traces: ecs.create-service-mismatched-launch-type-capacity-provider-rejected.

Cause 3 of 5: PutClusterCapacityProviders OVER-refuses — it raises ResourceInUseException when the only thing using the provider being dropped is a standalone RUNNING task.
  AWS: PutClusterCapacityProviders replaces a cluster's whole `capacityProviders` list and `defaultCapacityProviderStrategy`. Dropping a provider that is referenced only by an already-placed standalone task (one started by RunTask, not by a service) is ACCEPTED with HTTP 200 and leaves that task running; ResourceInUseException (Smithy: modeled on PutClusterCapacityProviders at httpError 400, documented "The specified resource is in-use and can't be removed.") is reserved for a provider an ACTIVE service still depends on. Recorded behavior for the accepted case: HTTP 200 with the updated cluster, the dropped provider gone from `capacityProviders`, and the task's `lastStatus` unchanged.
  Emulator: the call is refused — HTTP 400, code `ResourceInUseException`, message `capacity provider "FARGATE_SPOT" is still used by a task of cluster "<cluster>" and cannot be disassociated`. In that message the only fixed-text candidates are the emulator's own words; the provider name and cluster name are values taken from the request and the emulator's state, and AWS produces no such message for this case at all — it produces no error.
  Why: a client cannot roll a cluster's capacity-provider list forward while any standalone task is still alive; a legitimate 200 becomes a hard 400, and the condition AWS does refuse (an ACTIVE service's provider) becomes indistinguishable from the condition it allows.
  Fix: the PutClusterCapacityProviders handler's in-use validation and the cluster/capacity-provider association in `crate::state` — scope the ResourceInUseException predicate to non-INACTIVE SERVICES that name the provider, not to placed tasks.
  Traces: ecs.e2e-disassociate-capacity-provider-in-use-rejected.

Cause 4 of 5: a task placed through a capacityProviderStrategy carries `capacityProviderName` but no `launchType`.
  AWS: when RunTask places a task via `capacityProviderStrategy` naming FARGATE, the task carries BOTH fields — `capacityProviderName` equal to the provider and `launchType` DERIVED from it (`FARGATE`) — in the RunTask response and in DescribeTasks. Smithy: Task has both `capacityProviderName` (String, "The capacity provider that's associated with the task.") and `launchType` (LaunchType enum, "The infrastructure where your task runs on"); RunTask input accepts `capacityProviderStrategy` and `launchType`. The mirror rule is the discriminator: a task placed by `launchType` alone reports `launchType` and ECS OMITS `capacityProviderName` entirely.
  Emulator: `capacityProviderName` is stamped but `launchType` is absent — `AssertionError: a task placed by the FARGATE capacity provider must also report the launchType that provider implies, got None`.
  Why: a client that branches on `task.launchType` to decide Fargate-vs-EC2 behaviour (platform version handling, ENI expectations, pricing/reporting) sees an unset field for every capacity-provider-placed task and mis-classifies it.
  Fix: the RunTask handler's placement/stamping path and the Task record in `crate::state` — derive and persist `launchType` from the chosen capacity provider, and keep omitting `capacityProviderName` for a launchType-only placement.
  Traces: ecs.run-task-capacity-provider-strategy-conflict-rejected.

Cause 5 of 5: the zero-capacity capacity-provider scenario never converges — neither the refusal nor the control Fargate task is produced inside the budget.
  AWS: RunTask naming an ACTIVE, ASG-backed capacity provider whose Auto Scaling group has min=max=desired=0 is refused with HTTP 400 and code `InvalidParameterException`; the message's fixed text is `The specified capacity provider '<name>' contains no container instances.` with `<name>` the capacityProvider value taken from the request. Nothing materialises: ListTasks for the cluster stays empty and the check's log group holds no stream. The same task definition then runs on the SAME cluster's `FARGATE` provider: the task reaches `lastStatus: STOPPED` with `capacityProviderName: FARGATE`, its container exits 0, and exactly one awslogs stream `<prefix>/<container>/<task-id>` appears carrying the container's output and the task's own ARN. Smithy: RunTask errors include InvalidParameterException at httpError 400; the timing is recorded behavior only.
  Emulator: the body never reaches a verdict — `Error: Check timed out after 180.0s`. Neither leg completes: no refusal is returned for the zero-capacity provider and no control container runs to completion with its log stream, so a client gets neither an error nor a placed task.
  Why: the whole capacity-provider admission decision is unobservable. A client cannot learn that a provider has no capacity (AWS tells it synchronously, with the provider name in the message) and cannot observe a successful placement either; the call simply hangs from the client's point of view.
  Fix: the RunTask handler's capacity-provider admission check (refuse InvalidParameterException with AWS's fixed sentence when the named provider's ASG has no registered container instances) plus the Fargate placement/task-execution path that must drive the control task to STOPPED with exitCode 0 and emit its awslogs stream. Related open issues on the same execution gap, neither of which carries this check's replay key: 20261104T062603Z-ecs-containers-never-execute-no-in-container-behaviour.md and 20261105T143800Z-ecs-2-net-new-causes-containers-never-execute.md.
  Traces: ecs.capprov-zero-capacity-provider-rejects-task-and-no-container-runs.

## Evidence
Cause 1 — expected (recorded AWS): after `ec2 run-instances` with the ECS instance profile and `ECS_CLUSTER=<c>` user data, `ecs list-container-instances --cluster <c>` returns an ARN whose `describe-container-instances` entry has `ec2InstanceId` = the launched id; then `ecs update-container-instances-state --status DRAINING` -> 200 with `failures: []` and `status: DRAINING`, `--status REGISTERING` -> 400 `InvalidParameterException` / `Container instances status should be one of [ACTIVE,DRAINING]`, and `describe-container-instances` still reports DRAINING. Actual (emulator):
  Error: Check timed out after 180.0s   (ecs.update-container-instance-invalid-status-fails — cut inside `_launch_real_container_instance`'s `_poll_until(_find, ... description="EC2 instance registers as an ECS container instance", timeout_s=200)`)
  Error: Check timed out after 180.0s   (ecs.task-state-change-sequencing — same helper, same poll; its SubmitTaskStateChange assertions are never reached)

Cause 2 — expected: `ecs create-service --cluster <c> --service-name <s> --task-definition <td> --desired-count 0 --launch-type EC2 --capacity-provider-strategy capacityProvider=FARGATE,weight=1` -> HTTP 400, `Error.Code = InvalidParameterException`, `Error.Message = "Specifying both a launch type and capacity provider strategy is not supported. Remove one and try again."` (whole message fixed text). Actual:
  File "check_ecs_create_service_mismatched_launch_type_capacity_provider_rejected.py", line 59, in run
    assert message == expected_message
  AssertionError: expected message 'Specifying both a launch type and capacity provider strategy is not supported. Remove one and try again.', got 'You may choose a capacity provider or a launch type but not both.'

Cause 3 — expected: with FARGATE_SPOT associated and one standalone RUNNING task placed on it, `ecs put-cluster-capacity-providers --cluster <c> --capacity-providers FARGATE --default-capacity-provider-strategy capacityProvider=FARGATE,weight=1` -> HTTP 200, FARGATE_SPOT dropped, the task still RUNNING. Actual:
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.ResourceInUseException: An error occurred (ResourceInUseException) when calling the PutClusterCapacityProviders operation: capacity provider "FARGATE_SPOT" is still used by a task of cluster "sim2real-ecs-neginuse-<id>" and cannot be disassociated
(the provider name and the cluster name in that message are values from the request/state; the cluster suffix is a per-run id. The divergence is the 400 itself — AWS returns 200 here.)

Cause 4 — expected: `ecs run-task --cluster <c> --task-definition <td> --count 1 --capacity-provider-strategy capacityProvider=FARGATE,weight=1 --network-configuration ...` -> the RunTask response task and the later `describe-tasks` entry BOTH have `capacityProviderName: "FARGATE"` and `launchType: "FARGATE"`; a `--launch-type FARGATE` run of the same task definition has `launchType: "FARGATE"` and NO `capacityProviderName` key at all. Actual:
  File "check_ecs_run_task_capacity_provider_strategy_conflict_rejected.py", line 224, in run
    assert placed.get("launchType") == "FARGATE"
  AssertionError: a task placed by the FARGATE capacity provider must also report the launchType that provider implies, got None
(`capacityProviderName` was already asserted equal to "FARGATE" on the line above, so only `launchType` is missing.)

Cause 5 — expected: `ecs run-task --cluster <c> --task-definition <td> --count 1 --capacity-provider-strategy capacityProvider=<zero-capacity-asg-provider>,weight=1` -> HTTP 400, `Error.Code = InvalidParameterException`, `Error.Message = "The specified capacity provider '<provider>' contains no container instances."` (fixed text with the provider name from the request), `ecs list-tasks` empty and the log group holding no stream; then the same run with `capacityProvider=FARGATE` reaches `lastStatus: STOPPED`, `capacityProviderName: FARGATE`, container `exitCode: 0`, and exactly one awslogs stream containing the marker and the task ARN. Actual:
  Error: Check timed out after 180.0s   (ecs.capprov-zero-capacity-provider-rejects-task-and-no-container-runs)

## Steps to Reproduce
Cause 1:
1. `ecs create-cluster --cluster-name <c>`; create an ECS instance role + instance profile with AmazonEC2ContainerServiceforEC2Role.
2. `ssm get-parameter --name /aws/service/ecs/optimized-ami/amazon-linux-2023/recommended/image_id`; `ec2 run-instances --image-id <ami> --instance-type t3.micro --iam-instance-profile Arn=<profile> --subnet-id <s> --security-group-ids <sg> --user-data $(base64 'echo ECS_CLUSTER=<c> >> /etc/ecs/ecs.config')`.
3. Poll `ecs list-container-instances --cluster <c>` -> stays empty for the whole budget (AWS returns one ARN whose describe entry has ec2InstanceId=<i-...>).
4. With that instance: `ecs update-container-instances-state --cluster <c> --container-instances <ci> --status DRAINING` (must be 200/DRAINING) then `--status REGISTERING` (must be 400 InvalidParameterException "Container instances status should be one of [ACTIVE,DRAINING]") and `ecs describe-container-instances` must still say DRAINING. Also: run a short-lived task on it to STOPPED, then `ecs submit-task-state-change --cluster <c> --task <t> --status RUNNING` must not regress `lastStatus`, while the same call with an unknown cluster must fail ClientException "Cluster not found.".
Replay: ecs.update-container-instance-invalid-status-fails, ecs.task-state-change-sequencing.

Cause 2:
1. `ecs create-cluster --cluster-name <c>`; `ecs register-task-definition --family <f> --requires-compatibilities EC2 --container-definitions '[{"name":"app","image":"public.ecr.aws/amazonlinux/amazonlinux:2023","memory":128,"essential":true}]'`.
2. `ecs create-service --cluster <c> --service-name <s> --task-definition <td> --desired-count 0 --launch-type EC2 --capacity-provider-strategy capacityProvider=FARGATE,weight=1` -> compare `Error.Message` against AWS's fixed sentence.
3. `ecs describe-services --cluster <c> --services <s>` must return `services: []` plus a MISSING failure, and a launchType-only CreateService must still be accepted and ACTIVE.
Replay: ecs.create-service-mismatched-launch-type-capacity-provider-rejected.

Cause 3:
1. `ecs create-cluster --cluster-name <c>`; `iam create-service-linked-role --aws-service-name ecs.amazonaws.com`; `ecs put-cluster-capacity-providers --cluster <c> --capacity-providers FARGATE --default-capacity-provider-strategy capacityProvider=FARGATE,weight=1`.
2. Associate FARGATE_SPOT too and `ecs run-task --cluster <c> --task-definition <sleep-td> --count 1 --capacity-provider-strategy capacityProvider=FARGATE_SPOT,weight=1 --network-configuration ...`; wait for the task to be RUNNING.
3. `ecs put-cluster-capacity-providers --cluster <c> --capacity-providers FARGATE --default-capacity-provider-strategy capacityProvider=FARGATE,weight=1` -> emulator: 400 ResourceInUseException naming the provider and cluster; AWS: 200, FARGATE_SPOT dropped, the task still RUNNING.
Replay: ecs.e2e-disassociate-capacity-provider-in-use-rejected.

Cause 4:
1. Create a cluster with FARGATE associated and register a Fargate-compatible task definition.
2. `ecs run-task --cluster <c> --task-definition <td> --count 1 --capacity-provider-strategy capacityProvider=FARGATE,weight=1 --network-configuration 'awsvpcConfiguration={subnets=[...],securityGroups=[...]}'` -> inspect the response task and `ecs describe-tasks`: `launchType` is absent (AWS: "FARGATE").
3. `ecs run-task ... --launch-type FARGATE` -> `launchType: FARGATE` and `capacityProviderName` must be omitted entirely.
Replay: ecs.run-task-capacity-provider-strategy-conflict-rejected.

Cause 5:
1. Create an Auto Scaling group with min=max=desired=0 from a launch template using the ECS-optimized AMI; `ecs create-capacity-provider --name <p> --auto-scaling-group-provider autoScalingGroupArn=<asg>`; wait for it to be ACTIVE.
2. `ecs create-cluster --cluster-name <c> --capacity-providers <p> FARGATE`; create a log group and an execution role; register a task definition with the awslogs driver and `public.ecr.aws/docker/library/busybox:1.36`.
3. `ecs run-task --cluster <c> --task-definition <td> --count 1 --capacity-provider-strategy capacityProvider=<p>,weight=1 --network-configuration ...` -> must be 400 InvalidParameterException "The specified capacity provider '<p>' contains no container instances." with no task and no log stream created.
4. Same RunTask with `capacityProvider=FARGATE` -> must reach STOPPED with exitCode 0 and exactly one awslogs stream carrying the container output. Emulator: the sequence never converges inside the budget.
Replay: ecs.capprov-zero-capacity-provider-rejects-task-and-no-container-runs.
