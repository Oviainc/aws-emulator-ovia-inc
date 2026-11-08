Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__sqs.a-runtask-that-failed-placement-emits-no-task-state-change-event
Issue-Key: ecs__events__sqs.a-runtask-that-failed-placement-emits-no-task-state-change-event|_poll_until#42|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 4466ef5882b584b00b87b338b6098c1e65748c9e

# ecs__events__sqs: an awsvpc task's ENI attachment never leaves ATTACHED after the task reaches STOPPED, so the attachment's DETACHING/DETACHED/DELETED lifecycle is never observable

> _Note: this issue was identified against the code base at commit `4466ef5882b584b00b87b338b6098c1e65748c9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
Cause 1 of 1: an awsvpc (Fargate) task's `ElasticNetworkInterface` attachment keeps `status: ATTACHED` forever once the task has reached a STOPPED lastStatus; it never advances through the detach states, so any client that waits for the ENI to be released waits until its own deadline.

AWS: Smithy (service com.amazonaws.ecs, Task.attachments -> Attachment{id, type, status, details}; Attachment$status documentation: 'The status of the attachment. Valid values are PRECREATED, CREATED, ATTACHING, ATTACHED, DETACHING, DETACHED, DELETED, and FAILED.'). For an awsvpc task the attachment of type `ElasticNetworkInterface` is created and ATTACHED while the task runs, and once the task stops ECS releases the managed ENI: DescribeTasks reports the attachment moving ATTACHED -> DETACHING -> DETACHED and finally DELETED (the terminal state for a torn-down managed ENI), after which the underlying network interface no longer exists and the subnet/VPC can be deleted. The status strings are the modelled enumeration above; the attachment id, networkInterfaceId, macAddress and privateIPv4Address in `details` are per-task values.

Emulator: after the control task's lastStatus has reached STOPPED, DescribeTasks keeps returning a single attachment stuck at `'status': 'ATTACHED'` with its full detail set (subnetId, networkInterfaceId, macAddress, privateIPv4Address) for the whole 120s poll window — no DETACHING, no DETACHED, no DELETED, and the attachment is never removed. The check fails its bounded wait with: "the control task <task-arn>'s ENI attachment(s) reaching DELETED after STOPPED, so teardown does not race the network interface's own detachment not met within 120s."

Why: a client that gates teardown or reuse on ENI release — delete the subnet/security group/VPC only after the task's ENI is gone, reclaim an IP, or assert that a stopped awsvpc task holds no network resources — never observes the release. Waits never converge (they burn their whole budget and fail), and teardown that AWS orders after detachment races a network interface the emulator still reports as attached. It also makes the stopped-task attachment state indistinguishable from the running-task one.

Fix: ECS task lifecycle state for awsvpc tasks — when a task transitions to a STOPPED lastStatus, the attachment state machine in the task's state must advance its ElasticNetworkInterface attachment through DETACHING/DETACHED to DELETED (and release the backing network interface) so DescribeTasks reflects the terminal attachment status. This is not covered by the open issues about missing ECS Task State Change events (20261105T143253Z-ecs-events-sqs-a-stopped-fargate-task-publishes.md) or about the attachment's synthetic networkInterfaceId (20261105T143800Z-ec2-ecs-an-awsvpc-task-s-attachment-names.md); those address the event stream and the ENI id, not the attachment's post-STOPPED status transition.

Traces: ecs__events__sqs.a-runtask-that-failed-placement-emits-no-task-state-change-event — the bounded poll for the attachment reaching DELETED after STOPPED times out with the last observed attachment still ATTACHED.

## Evidence
Check: ecs__events__sqs.a-runtask-that-failed-placement-emits-no-task-state-change-event (verdict: mismatch).

Expected (AWS, per Attachment$status in the ECS Smithy model — valid values include DETACHING, DETACHED, DELETED): once the control awsvpc task's lastStatus is STOPPED, DescribeTasks shows its ElasticNetworkInterface attachment progressing off ATTACHED and reaching the terminal DELETED state (the ENI is released), so teardown of the subnet/VPC does not race the interface's own detachment.

Actual (emulator), last observed value after a 120s bounded wait:
  [{'id': '...', 'type': 'ElasticNetworkInterface', 'status': 'ATTACHED',
    'details': [{'name': 'subnetId', ...}, {'name': 'networkInterfaceId', ...},
                {'name': 'macAddress', ...}, {'name': 'privateIPv4Address', ...}]}]
AssertionError: "the control task <task-arn>'s ENI attachment(s) reaching DELETED after STOPPED, so teardown does not race the network interface's own detachment not met within 120s. Last result: [...status 'ATTACHED'...]".
The attachment id, eni id, mac, IP and subnet id in that body are per-run values; the requirement is that `status` leaves ATTACHED and reaches DELETED after the task stops.

## Steps to Reproduce
1. Create a VPC and one subnet (no IGW/NAT needed) and an ECS cluster.
2. RegisterTaskDefinition for a Fargate/awsvpc task definition.
3. RunTask with launchType FARGATE and an awsvpcConfiguration naming that subnet (a control task that gets placed).
4. Poll DescribeTasks until the task's lastStatus is STOPPED.
5. Keep polling DescribeTasks and read the task's `attachments` entry of type ElasticNetworkInterface.
   Expected: its `status` leaves ATTACHED (DETACHING/DETACHED) and reaches DELETED within the poll window, and the backing network interface is released.
   Observed: `status` stays 'ATTACHED' indefinitely (still ATTACHED after 120s), so a wait for DELETED never converges.
