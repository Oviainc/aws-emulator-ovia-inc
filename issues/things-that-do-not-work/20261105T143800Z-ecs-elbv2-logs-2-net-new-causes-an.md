Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__elbv2__logs.ecs-service-behind-alb-serves-http-request ecs__elbv2__logs.ecs-update-service-to-a-different-network-mode-rejected
Issue-Key: ecs__elbv2__logs.ecs-service-behind-alb-serves-http-request|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: ecs__elbv2__logs.ecs-update-service-to-a-different-network-mode-rejected|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# ecs__elbv2__logs: 2 net-new causes — an ALB-fronted ECS service never answers an HTTP request, and the awsvpc→bridge UpdateService refusal cannot be exercised

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateService

## Description
Two net-new root causes; neither check was marked ALREADY FILED or OWNED. The open issue 20261104T062603Z-ecs-elbv2-logs-an-awsvpc-task-never-gets.md covers an adjacent cause under this heading but does not carry these two replay keys.

Cause 1 of 2: a GET to the ALB's DNS name never returns the body the ECS service's task wrote at startup, and the container's stdout never reaches CloudWatch Logs.
AWS: once the service converges, its awsvpc task's private IPv4 is registered as an `ip` target, passes health checks and the listener forwards, so an HTTP GET to the load balancer's DNSName returns exactly the body the container wrote; the same container's stdout is delivered by the awslogs driver into the configured log group. Recorded behavior only for timing; the end-to-end response body is what the recording shows.
Emulator: the check is killed with "Check timed out after 60.0s" while polling — the ALB endpoint never serves the task's body and no log event is delivered, so neither convergence, registration, health nor forwarding can be confirmed.
Why: the primary thing a user builds this stack for — an HTTP response from their container through the load balancer — never happens, and there is no log output to diagnose it with.
Fix: ECS task-attachment state (real ENI private IPv4), the ECS→ELBv2 `ip` target registration, the ELBv2 listener's forwarding to a registered target, and the awslogs delivery path.
Traces: ecs__elbv2__logs.ecs-service-behind-alb-serves-http-request.

Cause 2 of 2: the modeled refusal of a network-mode change on a live ALB-attached service is unreachable, because the service never becomes live.
AWS: UpdateService that points a live awsvpc, ALB-attached Fargate service at a bridge-mode revision is refused with InvalidParameterException (Smithy: ECS InvalidParameterException, httpError 400) — the message's fixed text states that the task definition's network mode is incompatible with the service's load balancer / awsvpc configuration, with the service name, the task definition ARN and the network mode taken from the request and the service record. The service keeps answering HTTP through its load balancer throughout, and the same call succeeds once only the network mode is repaired.
Emulator: the check is killed with "Check timed out after 60.0s" before the refusal can be attempted, because the service never serves HTTP through the ALB (Cause 1). The refusal, the preserved-availability invariant and the post-repair success are therefore all unverified here.
Why: a client relying on ECS to refuse an incompatible network-mode change may instead have it accepted (unknown here) and in either case cannot observe that its service stayed available during the refused update.
Fix: on top of Cause 1, UpdateService validation must compare the target revision's networkMode against the service's existing loadBalancers/awsvpc configuration and refuse with InvalidParameterException.
Traces: ecs__elbv2__logs.ecs-update-service-to-a-different-network-mode-rejected.

## Evidence
Expected (AWS): `GET http://<alb-dns-name>/` returns the exact body the task wrote at startup and the container's stdout appears in the configured CloudWatch Logs group; UpdateService onto a bridge-mode revision is refused with InvalidParameterException (HTTP 400) while the service keeps serving, and succeeds after the network mode is repaired.
Actual (emulator): both checks are terminated by the harness with `Check timed out after 60.0s` — the ALB never serves the task's body, no log event is delivered, and the UpdateService refusal is never reached.

## Steps to Reproduce
1. Create a VPC with ≥2 subnets, an ALB, an `ip` target group, and a CloudWatch Logs group.
2. RegisterTaskDefinition (awsvpc/FARGATE, awslogs driver) whose container writes a known body and serves it over HTTP.
3. CreateService with loadBalancers bound to the target group, desiredCount=1.
4. GET the ALB DNSName and poll the log group → neither the body nor any log event ever appears (check times out at 60s).
5. RegisterTaskDefinition a bridge-mode revision and call UpdateService onto it → never reached, because step 4 never settles.
