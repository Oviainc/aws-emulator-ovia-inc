Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__logs.log-vpc-flow-logs-scope-excludes-non-member-eni ec2__iam__logs.log-vpc-flow-logs-traffic-type-filter-excludes-other-action ec2__iam__logs.xsvc-ec2-3-flow-log-delivery-role-without-put-log-events-denied
Issue-Key: ec2__iam__logs.log-vpc-flow-logs-scope-excludes-non-member-eni|run#27|PublishedUrlRejected|(ctx.endpoint_host(ip_b, p_b)[0], p_b, sp_b),
Issue-Key: ec2__iam__logs.log-vpc-flow-logs-traffic-type-filter-excludes-other-action|run#29|PublishedUrlRejected|(ctx.endpoint_host(ip_server, p_ok)[0], p_ok, sp_ok),
Issue-Key: ec2__iam__logs.xsvc-ec2-3-flow-log-delivery-role-without-put-log-events-denied|run#26|PublishedUrlRejected|(eni_client, _client_userdata([(ctx.endpoint_host(ip_server, port)[0], port, sport)])),
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# VPC flow-log checks cannot run: the emulator publishes an ENI private IP (10.x.x.x:<port>) as the endpoint host for in-VPC traffic

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlowLogs

## Description
Cause 1 of 1: the address the emulator announces for an in-VPC endpoint is a private VPC IP it does not serve.
AWS: traffic driven to an instance/ENI's private address inside the VPC is recorded by the flow log, so a client can assert that the flow log's scope excludes a non-member ENI, that the TrafficType filter excludes the other action, and that a delivery role without logs:PutLogEvents writes nothing (Smithy ec2 CreateFlowLogs).
Emulator: the endpoint host is published as '10.84.2.4:27870' (and 10.83.1.5:24924, 10.95.1.5:26462) — 'its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address' — so no traffic can be generated and no flow-log assertion is reachable.
Why: a client that reads the ENI's published address and sends traffic to it reaches nothing; every VPC flow-log scenario (scope, traffic-type filtering, delivery-permission denial) is unobservable.
Fix: the published endpoint host for in-VPC resources (the ENI/instance address serializer must announce an address the emulator actually listens on, or a loopback address mapped to it).
Traces: the three checks in trace_ids.

## Evidence
'simparity.published.PublishedUrlRejected: endpoint_host was given 10.84.2.4:27870 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]' (and the same for 10.83.1.5:24924 and 10.95.1.5:26462).

## Steps to Reproduce
Replay ec2__iam__logs.log-vpc-flow-logs-scope-excludes-non-member-eni, .log-vpc-flow-logs-traffic-type-filter-excludes-other-action and .xsvc-ec2-3-flow-log-delivery-role-without-put-log-events-denied: each creates a VPC/ENI, reads the endpoint host it must send traffic to, and is refused the published private address.
