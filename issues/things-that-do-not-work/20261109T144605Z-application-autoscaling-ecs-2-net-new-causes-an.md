Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.step-scaling-selects-the-step-at-an-exact-interval-boundary application-autoscaling__ecs.step-scaling-three-adjustment-types-from-one-capacity application-autoscaling__ecs.suspended-scaling-dimensions-are-independent-not-one-switch application-autoscaling__ecs.step-scaling-scale-in-cooldown-suppresses-then-permits
Issue-Key: application-autoscaling__ecs.step-scaling-selects-the-step-at-an-exact-interval-boundary|_poll_until#8|AssertionError|raise AssertionError(f'{description} not observed within {timeout_s}s; last saw {last!r}')
Issue-Key: application-autoscaling__ecs.step-scaling-three-adjustment-types-from-one-capacity|_poll_until#8|AssertionError|raise AssertionError(f'{description} not observed within {timeout_s}s; last saw {last!r}')
Issue-Key: application-autoscaling__ecs.suspended-scaling-dimensions-are-independent-not-one-switch|_poll_until#11|AssertionError|raise AssertionError(f'{description} not observed within {timeout_s}s; last saw {last!r}')
Issue-Key: application-autoscaling__ecs.step-scaling-scale-in-cooldown-suppresses-then-permits|run#44|AssertionError|assert _alarm_state(cw, in_alarm) == 'ALARM', (
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# application-autoscaling__ecs: 2 net-new causes — an ECS-user-data EC2 instance never registers as a container instance, and a CloudWatch alarm does not hold ALARM across its suppression window

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterContainerInstance

## Description
Cause 1 of 2: an EC2 instance launched with ECS-cluster user data never appears as an ACTIVE ECS container instance, so every step-scaling check that needs EC2 capacity stalls at setup.
AWS: recorded behavior only. The checks launch an EC2 instance whose user data writes ECS_CLUSTER=<cluster> and then poll ECS ListContainerInstances/DescribeContainerInstances until the instance id shows up with status ACTIVE. On real AWS the container agent calls ECS RegisterContainerInstance (Smithy: RegisterContainerInstance, errors ClientException/InvalidParameterException/ServerException) and the instance becomes ACTIVE well inside the 240s budget the checks allow.
Emulator: `AssertionError: EC2 instance 'i-00000000000000001' registering as an ACTIVE ECS container instance on cluster '<cluster>' not observed within 240.0s; last saw None`. The instance exists in EC2 but ECS never learns about it, so the cluster has zero registered capacity forever.
Why: any EC2-launch-type ECS scenario is unreachable. A client cannot build an EC2-backed cluster at all, so task placement, capacity providers and EC2 step scaling can never be exercised.
Fix: the bridge between EC2 RunInstances and ECS container-instance registration — either the emulator's simulated ECS agent or the state that RegisterContainerInstance/ListContainerInstances read, in the ECS crate's container-instance state and its register_container_instance/list_container_instances handlers.
Traces: application-autoscaling__ecs.step-scaling-selects-the-step-at-an-exact-interval-boundary, application-autoscaling__ecs.step-scaling-three-adjustment-types-from-one-capacity, application-autoscaling__ecs.suspended-scaling-dimensions-are-independent-not-one-switch. (The open issue 20261109T083521Z-ecs-ec2-instances-launched-with-ecs-cluster-user.md describes the same shape under the `ecs` heading; it does not carry these replay keys.)

Cause 2 of 2: a CloudWatch alarm does not stay in ALARM for the duration of a scale-in cooldown window, so the cooldown cannot be observed.
AWS: recorded behavior only. An alarm whose metric keeps breaching stays in ALARM until a non-breaching datapoint (or INSUFFICIENT_DATA) is evaluated; Application Auto Scaling's scale-in cooldown then suppresses a second scale-in while the alarm is still ALARM, and permits it once the cooldown elapses. Smithy (cloudwatch): DescribeAlarms returns MetricAlarm.StateValue in {OK, ALARM, INSUFFICIENT_DATA}.
Emulator: `AssertionError: leg 2: the scale-in alarm 'sim2real-behav-aas-coolinal-<id>' left ALARM during the suppression window, so an unchanged capacity would prove nothing about the cooldown`. The alarm flips out of ALARM on its own while the breaching metric is still being fed, which destroys the premise of the cooldown test.
Why: alarm state is not stable under a steady breaching metric, so any client that keys behaviour off a sustained ALARM (autoscaling cooldowns, paging, remediation) sees spurious recovery. The fixed text of the requirement is the state machine, not a message: StateValue must remain 'ALARM' while every evaluated datapoint in the evaluation window breaches the threshold.
Fix: CloudWatch alarm evaluation — the alarm evaluator must re-evaluate from the ingested datapoints in the evaluation window and keep StateValue at ALARM while they breach, instead of decaying to OK/INSUFFICIENT_DATA.
Traces: application-autoscaling__ecs.step-scaling-scale-in-cooldown-suppresses-then-permits.

## Evidence
Cause 1 — Expected: ECS DescribeContainerInstances reports the launched instance id with status 'ACTIVE' within 240s. Actual: `AssertionError: EC2 instance 'i-00000000000000001' registering as an ACTIVE ECS container instance on cluster 'sim2real-behav-aas-bound-cl-<id>' not observed within 240.0s; last saw None` (identical for the -adj- and -suspsub- clusters).
Cause 2 — Expected: DescribeAlarms keeps StateValue='ALARM' for the whole scale-in suppression window while the breaching metric is fed. Actual: `AssertionError: leg 2: the scale-in alarm '<alarm>' left ALARM during the suppression window, so an unchanged capacity would prove nothing about the cooldown`.

## Steps to Reproduce
Cause 1: ec2 RunInstances with user data `#!/bin/bash\necho ECS_CLUSTER=<cluster> >> /etc/ecs/ecs.config`, then poll ecs list_container_instances/describe_container_instances for the instance id with status ACTIVE. It never appears.
Cause 2: register an ecs:service:DesiredCount scalable target with a step scale-in policy and a scale-in cooldown; feed a continuously breaching metric with cloudwatch PutMetricData and poll DescribeAlarms through the cooldown window. The alarm leaves ALARM mid-window.
