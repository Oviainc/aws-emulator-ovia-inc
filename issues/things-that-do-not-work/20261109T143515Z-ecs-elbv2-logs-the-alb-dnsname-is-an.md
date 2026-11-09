Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__elbv2__logs.ecs-service-behind-alb-serves-http-request ecs__elbv2__logs.ecs-update-service-to-a-different-network-mode-rejected ecs__elbv2__logs.alb-unhealthy-only-target-fails-open-empty-group-returns-503 ecs__elbv2__logs.ecs-container-health-check-failure-makes-the-service-replace-the-task-and-the-alb-drop-the-old-target
Issue-Key: ecs__elbv2__logs.ecs-service-behind-alb-serves-http-request|_http_get#25|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: ecs__elbv2__logs.ecs-update-service-to-a-different-network-mode-rejected|_http_get#25|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: ecs__elbv2__logs.alb-unhealthy-only-target-fails-open-empty-group-returns-503|_poll_until#17|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: ecs__elbv2__logs.ecs-container-health-check-failure-makes-the-service-replace-the-task-and-the-alb-drop-the-old-target|_poll_until#22|AssertionError|raise AssertionError(
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# ecs__elbv2__logs: the ALB DNSName is an elb.amazonaws.com host, awslogs streams never appear, and a container's healthStatus never leaves UNKNOWN (3 causes)

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
Cause 1 of 3: the load balancer publishes an AWS production hostname.
AWS: LoadBalancer.DNSName is the host a client requests, and the recorded runs serve HTTP through it and assert the network-mode update refusal only after reaching the service.
Emulator: it publishes 'http://<lb>-<id>.us-west-2.elb.amazonaws.com/', which the emulator does not serve — 'its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>'.
Why: the ALB is unreachable for any client honouring DNSName, so serving traffic through an ECS service behind an ALB is impossible.
Fix: the DNSName serializer/state in the elbv2 crate.
Traces: ecs__elbv2__logs.ecs-service-behind-alb-serves-http-request, ecs__elbv2__logs.ecs-update-service-to-a-different-network-mode-rejected.

Cause 2 of 3: the awslogs driver never creates the task's log stream.
AWS: a task whose container uses the awslogs driver writes to '<prefix>/<container>/<task-id>' in the configured group as soon as it starts.
Emulator: 'awslogs stream ecs/app/<task-id> in <group> not met within 300s. Last result: []' — the stream is never created and no event is delivered.
Why: container output is lost; a client debugging a task sees an empty log group.
Fix: the awslogs log driver delivery path in the ecs crate.
Traces: ecs__elbv2__logs.alb-unhealthy-only-target-fails-open-empty-group-returns-503.

Cause 3 of 3: a container's healthStatus never reflects its health check.
AWS: a container with a healthCheck transitions healthStatus to HEALTHY (then UNHEALTHY on failure), which drives service replacement and ALB target deregistration.
Emulator: healthStatus stays 'UNKNOWN' — 'container healthStatus HEALTHY on <task arn> not met within 420.0s. Last result: 'UNKNOWN''.
Why: health-check-driven service replacement and target draining never happen, so a client's rolling-deployment safety net does not exist.
Fix: container health-check evaluation in the ecs task state.
Traces: ecs__elbv2__logs.ecs-container-health-check-failure-makes-the-service-replace-the-task-and-the-alb-drop-the-old-target.

## Evidence
Cause 1 — 'simparity.published.PublishedUrlRejected: endpoint_url was given http://sim2real-xsvc-lb-<id>-<id>.us-west-2.elb.amazonaws.com/ ... its host is not one of the emulator's own hosts' (two checks).
Cause 2 — 'AssertionError: awslogs stream ecs/app/00000000000000000000000000000001 in <group> not met within 300s. Last result: []'.
Cause 3 — 'AssertionError: container healthStatus HEALTHY on <task arn> not met within 420s. Last result: 'UNKNOWN''.

## Steps to Reproduce
Replay the four ecs__elbv2__logs checks in trace_ids.
