Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.awsvpc-task-attachment-details-match-network-configuration ecs.eni-attachment-reaches-attached ecs.submit-attachment-state-change-updates-attachment-status ecs.submit-attachment-state-changes-updates-eni-status ecs.service-converges-running-count-to-desired
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.awsvpc-task-attachment-details-match-network-configuration|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Key: ecs.eni-attachment-reaches-attached|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: ecs.submit-attachment-state-change-updates-attachment-status|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: ecs.submit-attachment-state-changes-updates-eni-status|_poll_for_attachment_status#8|Timeout|time.sleep(interval)
Issue-Key: ecs.service-converges-running-count-to-desired|run#28|AssertionError|assert task.get("connectivity") == "CONNECTED", (
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ECS awsvpc task ENIs never attach: the ElasticNetworkInterface attachment stays PRECREATED with no networkInterfaceId/privateIPv4Address and Task.connectivity is never set (1 cause)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 1: the awsvpc elastic-network-interface attachment an ECS task gets is created as a placeholder and never progresses.

AWS: Smithy com.amazonaws.ecs#Attachment.status documents the valid values PRECREATED, CREATED, ATTACHING, ATTACHED, DETACHING, DETACHED, DELETED, FAILED; Attachment.details for an elastic network interface "includes the network interface ID, the MAC address, the subnet ID, and the private IPv4 address". Smithy com.amazonaws.ecs#Task.connectivity (enum Connectivity) is the connectivity status of a task and Task.connectivityAt the time it last went CONNECTED. On real AWS an awsvpc task that reaches lastStatus RUNNING carries exactly one attachment of type ElasticNetworkInterface with status ATTACHED whose details contain at least networkInterfaceId, macAddress, subnetId and privateIPv4Address, and the task carries connectivity=CONNECTED.

Emulator: every RunTask/CreateService-launched awsvpc task is returned, and continues to be described, with attachments=[{id, type: 'ElasticNetworkInterface', status: 'PRECREATED', details: [{name: 'subnetId', value: <a subnet id>}]}] — status never leaves PRECREATED, details never gains networkInterfaceId/macAddress/privateIPv4Address, and the task object has no connectivity member at all (checks read None). Checks that wait for the attachment to reach ATTACHED, that read the ENI id back through EC2, or that submit an attachment state change and re-read it, poll until the 60s budget expires.

Why: a client cannot find the IP its task is reachable on, cannot join the ECS view of a task to the EC2 ENI, and cannot tell a connected task from a disconnected one. Every awsvpc-dependent integration (ALB/NLB ip targets, Service Connect, Cloud Map A-record registration, direct task-to-task traffic) is unreachable, and SubmitAttachmentStateChanges is unobservable.

Fix: the ECS task-state/attachment model — the awsvpc attachment is built once at launch in the RunTask/CreateService task-materialisation path and never advanced; the ENI needs to be allocated against the named subnet (producing networkInterfaceId, macAddress, privateIPv4Address), the attachment driven PRECREATED -> ATTACHED, Task.connectivity/connectivityAt set when the task reaches RUNNING, and SubmitAttachmentStateChanges must mutate that record.

Traces: the failing checks listed under trace_ids. Note the open issue 20261104T042502Z-autoscaling-ecs-elbv2-6-net-new-root-causes.md mentions a related 'task ENIs carry no private IP' symptom for the autoscaling__ecs__elbv2 suite; it does not carry these ecs replay keys.

## Evidence
Expected (AWS): attachments=[{type:'ElasticNetworkInterface', status:'ATTACHED', details:[{name:'subnetId',...},{name:'networkInterfaceId',...},{name:'macAddress',...},{name:'privateIPv4Address',...}]}] and task.connectivity='CONNECTED'.
Actual (emulator): attachments=[{'id': '00000001-0000-0000-0000-000000000001', 'type': 'ElasticNetworkInterface', 'status': 'PRECREATED', 'details': [{'name': 'subnetId', 'value': 'subnet-0000000000000003e'}]}] (the id/subnet values are per-run; the shape is the defect) and no connectivity member.
Replay details: `ecs.service-converges-running-count-to-desired` -> AssertionError: a RUNNING awsvpc task must report connectivity CONNECTED, got None. `ecs.eni-attachment-reaches-attached`, `ecs.awsvpc-task-attachment-details-match-network-configuration`, `ecs.submit-attachment-state-change-updates-attachment-status`, `ecs.submit-attachment-state-changes-updates-eni-status` -> Check timed out after 60.0s.

## Steps to Reproduce
1. CreateCluster, RegisterTaskDefinition with networkMode=awsvpc and requiresCompatibilities=[FARGATE].
2. RunTask with launchType=FARGATE and networkConfiguration.awsvpcConfiguration.subnets=[<subnet>].
3. DescribeTasks on the returned taskArn and poll: the single attachment stays status=PRECREATED, details holds only subnetId, and the task never reports connectivity.
4. SubmitAttachmentStateChanges for that attachment and re-DescribeTasks: the status is unchanged.
