Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__elbv2__logs.alb-health-check-reflects-ecs-task-health cloudwatch__ecs__elbv2__logs.alb-request-count-metric-in-cloudwatch cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets cloudwatch__ecs__elbv2__logs.alb-target-response-time-metric-in-cloudwatch cloudwatch__ecs__elbv2__logs.ecs-service-cpu-memory-metrics-in-cloudwatch
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-health-check-reflects-ecs-task-health|_http_get#22|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-request-count-metric-in-cloudwatch|_generate_http_traffic#13|PublishedUrlRejected|_url, _route = ctx.endpoint_route(f"http://{dns_name}/")
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets|_http_get#28|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-target-response-time-metric-in-cloudwatch|_generate_http_traffic_with_latency#14|PublishedUrlRejected|target, route = ctx.endpoint_route(f"http://{dns_name}/")
Issue-Key: cloudwatch__ecs__elbv2__logs.ecs-service-cpu-memory-metrics-in-cloudwatch|_poll_metric_datapoints#11|Timeout|time.sleep(30)
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# An ALB's DNSName is published as <name>.<region>.elb.amazonaws.com, and ECS service CPU/memory metrics are never published (2 causes)

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
Cause 1 of 2: the load balancer announces an AWS production hostname rather than the address it is reached on.
AWS: LoadBalancer.DNSName is the host a client sends requests to; the recorded runs drive traffic through that name to observe target health, routing and the AWS/ApplicationELB request/latency metrics (Smithy elasticloadbalancingv2 LoadBalancer.DNSName).
Emulator: it publishes 'http://<lb-name>-<id>.us-west-2.elb.amazonaws.com/', a host the emulator does not serve, so no request can be made — 'its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>'.
Why: every ALB the emulator creates is unreachable for a client that honours DNSName, so health-check reflection, healthy-target-only routing, RequestCount and TargetResponseTime are all unobservable.
Fix: the DNSName serializer/state in the elbv2 crate (publish the emulator's own host and port for the load balancer's listener).
Traces: cloudwatch__ecs__elbv2__logs.alb-health-check-reflects-ecs-task-health, .alb-request-count-metric-in-cloudwatch, .alb-routes-traffic-only-to-healthy-ecs-targets, .alb-target-response-time-metric-in-cloudwatch.

Cause 2 of 2: AWS/ECS service metrics are never published, so the check never settles.
AWS: a running ECS service publishes AWS/ECS CPUUtilization and MemoryUtilization dimensioned by ClusterName and ServiceName.
Emulator: 'Check timed out after 900.0s' — the metric series never appears.
Why: a client's service dashboards and target-tracking autoscaling have no data for a running service.
Fix: the AWS/ECS service metric publisher in the ecs/cloudwatch state.
Traces: cloudwatch__ecs__elbv2__logs.ecs-service-cpu-memory-metrics-in-cloudwatch.

## Evidence
Cause 1 — 'simparity.published.PublishedUrlRejected: endpoint_url was given http://sim2real-alb-<id>-<id>.us-west-2.elb.amazonaws.com/ (published by the emulator or built by the check): its host is not one of the emulator's own hosts' (four checks).
Cause 2 — 'Error: Check timed out after 900.0s'.

## Steps to Reproduce
Replay the five cloudwatch__ecs__elbv2__logs checks in trace_ids: the first four read the ALB's DNSName and request it; the last polls GetMetricStatistics for AWS/ECS CPUUtilization of the running service.
