Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__elbv2__logs.ecs-service-behind-alb-serves-http-request ecs__elbv2__logs.ecs-update-service-to-a-different-network-mode-rejected
Issue-Key: ecs__elbv2__logs.ecs-service-behind-alb-serves-http-request|run#22|AssertionError|assert ip, f"task has no awsvpc private IP: {tasks[0].get('attachments')}"
Issue-Key: ecs__elbv2__logs.ecs-update-service-to-a-different-network-mode-rejected|run#22|AssertionError|assert ip, f"the task has no awsvpc private IP: {tasks[0].get('attachments')!r}"
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ecs__elbv2__logs: an awsvpc task never gets a private IP, so an ECS service behind an ALB serves nothing and the network-mode change check cannot run (1 cause)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
Cause 1 of 1: the awsvpc task attachment never carries a private IPv4 address, so there is no address to register behind the load balancer or to compare a network-mode change against.

AWS: Smithy com.amazonaws.ecs#Attachment.details documents that an ElasticNetworkInterface attachment includes the network interface ID, the MAC address, the subnet ID and the private IPv4 address. A client reads privateIPv4Address to register the task as an `ip` target and to reach it directly.

Emulator: the attachment is returned as [{'id': ..., 'type': 'ElasticNetworkInterface', 'status': 'PRECREATED', 'details': [{'name': 'subnetId', 'value': <subnet>}]}] — subnetId only, no privateIPv4Address — for the whole life of the task, so both checks fail outright on the missing address.

Why: an ECS service behind an ALB can never serve a request in the emulator, and a client cannot discover where its task is listening.

Fix: the ECS awsvpc ENI allocation/attachment materialisation (the same defect as the ecs attachment issue in this sweep): allocate the ENI against the named subnet and publish networkInterfaceId/macAddress/privateIPv4Address in the attachment details.

Traces: ecs__elbv2__logs.ecs-service-behind-alb-serves-http-request, ecs__elbv2__logs.ecs-update-service-to-a-different-network-mode-rejected.

## Evidence
`AssertionError: task has no awsvpc private IP: [{'id': '00000001-0000-0000-0000-000000000001', 'type': 'ElasticNetworkInterface', 'status': 'PRECREATED', 'details': [{'name': 'subnetId', 'value': 'subnet-0000000000000003e'}]}]` and `AssertionError: the task has no awsvpc private IP: [...same shape...]`. The ids/subnet are per-run; the missing privateIPv4Address (and the PRECREATED status) is the defect.

## Steps to Reproduce
1. CreateService with networkMode=awsvpc behind an ALB target group of type ip.
2. DescribeTasks for the service's task and read attachments[0].details for privateIPv4Address -> absent.
3. Attempt an HTTP request through the ALB -> nothing is served.
