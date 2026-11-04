Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs__logs.deep-ecs-1-guestdepth-ebs-runtask-without-volume-configuration-rejected ec2__ecs__logs.deep-ecs-1-guestdepth-ebs-volume-configured-at-launch ec2__ecs__logs.deep-ecs-1-guestdepth-firelens-fluentbit-routes-app-output ec2__ecs__logs.deep-ecs-1-guestdepth-pidmode-task
Issue-Key: ec2__ecs__logs.deep-ecs-1-guestdepth-ebs-runtask-without-volume-configuration-rejected|run#13|AssertionError|assert "accepted" not in outcome, (
Issue-Key: ec2__ecs__logs.deep-ecs-1-guestdepth-ebs-volume-configured-at-launch|run#22|AssertionError|assert len(vols) == 1, f"expected exactly one volume tagged {run_tag!r}, got {vols!r}"
Issue-Key: ec2__ecs__logs.deep-ecs-1-guestdepth-firelens-fluentbit-routes-app-output|_poll_until#14|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: ec2__ecs__logs.deep-ecs-1-guestdepth-pidmode-task|_poll_until#13|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ec2__ecs__logs: RunTask accepts a configuredAtLaunch volume with no volumeConfigurations, the EBS volume is never created, and FireLens/pidMode tasks never reach a terminal status (3 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 3: RunTask accepts a task definition with a configuredAtLaunch volume but no volumeConfigurations.
AWS: Smithy com.amazonaws.ecs#RunTaskRequest.volumeConfigurations — "The details of the volume that was configuredAtLaunch ... The name of the volume must match the name from the task definition" — and a task definition declaring configuredAtLaunch=true cannot be run without supplying it; AWS refuses with InvalidParameterException.
Emulator: HTTP 200, the task is created (lastStatus PENDING) with no volume configuration at all.
Fix: RunTask volume validation (match each configuredAtLaunch volume against volumeConfigurations by name).
Trace: ec2__ecs__logs.deep-ecs-1-guestdepth-ebs-runtask-without-volume-configuration-rejected.

Cause 2 of 3: a volume supplied through volumeConfigurations is never provisioned.
AWS: ECS creates the managed EBS volume (tagged for the task) and attaches it; EC2 DescribeVolumes finds exactly one volume carrying the task's tag.
Emulator: `AssertionError: expected exactly one volume tagged '<tag>', got []` — no EBS volume is created.
Fix: the ECS managed-EBS volume provisioning path (create/tag/attach the volume in EC2 state and record the AmazonElasticBlockStorage attachment on the task).
Trace: ec2__ecs__logs.deep-ecs-1-guestdepth-ebs-volume-configured-at-launch.

Cause 3 of 3: tasks exercising FireLens routing and pidMode never reach STOPPED.
AWS: the task runs and reaches lastStatus=STOPPED, with the FireLens router having forwarded the app container's output and the pidMode setting visible to the processes.
Emulator: `TimeoutError: task <taskArn> lastStatus in ['STOPPED']: the run budget ended first; not graded` — the task never terminates (and the container never runs).
Fix: the ECS task runtime (and, for FireLens, the log-router sidecar wiring).
Traces: ec2__ecs__logs.deep-ecs-1-guestdepth-firelens-fluentbit-routes-app-output, ec2__ecs__logs.deep-ecs-1-guestdepth-pidmode-task.

## Evidence
1. `AssertionError: RunTask WITHOUT volumeConfigurations was accepted for a configuredAtLaunch volume: {'tasks': [{... 'lastStatus': 'PENDING' ...}], 'failures': [], 'ResponseMetadata': {'HTTPStatusCode': 200 ...}}`.
2. `AssertionError: expected exactly one volume tagged 'sim2real-ebsrun-<suffix>', got []` (the tag value is per-run).
3. `TimeoutError: task arn:aws:ecs:us-west-2:000000000000:task/<cluster>/<id> lastStatus in ['STOPPED']: the run budget ended first; not graded` (x2).

## Steps to Reproduce
1. RegisterTaskDefinition with volumes=[{name:'data', configuredAtLaunch:true}]; RunTask with no volumeConfigurations -> 200.
2. RunTask the same definition WITH volumeConfigurations (managed EBS, tagged), then EC2 DescribeVolumes by that tag -> empty.
3. RunTask a FireLens (fluentbit router + app) task and a pidMode task and poll DescribeTasks for lastStatus=STOPPED -> never reached.
