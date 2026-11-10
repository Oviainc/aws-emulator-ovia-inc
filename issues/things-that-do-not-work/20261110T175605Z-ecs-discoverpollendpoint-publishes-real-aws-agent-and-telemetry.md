Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-agent-apis-and-exec-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-agent-apis-and-exec-error-paths ecs-cluster-setup-poll-endpoint-and-teardown
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DiscoverPollEndpoint|2xx->2xx|$.endpoint
Issue-Key: ecs|DiscoverPollEndpoint|2xx->2xx|$.telemetryEndpoint
Issue-Related: 20261105T015737Z-ecs-responses-diverge-service-connect-endpoint-failure-arn.md
Issue-Commit: 54b3fe10738e925f02de63999e233b2f22231166

# ECS DiscoverPollEndpoint publishes real AWS agent and telemetry hosts the emulator does not serve

> _Note: this issue was identified against the code base at commit `54b3fe10738e925f02de63999e233b2f22231166`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DiscoverPollEndpoint

## Description
One net-new ECS wire cause.

NOTE ON A NEIGHBOURING OPEN ISSUE: 20261105T015737Z-ecs-responses-diverge-service-connect-endpoint-failure-arn.md (its cause 3) covers only $.serviceConnectEndpoint of this operation. The cluster below now also fails on $.endpoint and $.telemetryEndpoint with the distinct verdict host_not_own, so it is not owned by that issue's replay key and must be re-replayed on its own.

Cause 1 of 1: DiscoverPollEndpoint answers with real-AWS host names instead of the address the emulator is actually reached on (operation DiscoverPollEndpoint; fixture ecs-agent-apis-and-exec-error-paths, step task#2).
AWS: Smithy com.amazonaws.ecs DiscoverPollEndpoint -> DiscoverPollEndpointResponse {endpoint, telemetryEndpoint, serviceConnectEndpoint}; endpoint is "The endpoint for the Amazon ECS agent to poll", telemetryEndpoint is "The telemetry endpoint for the Amazon ECS agent", and serviceConnectEndpoint is "The endpoint for the Amazon ECS agent to poll for Service Connect configuration". The contract of all three members is that they address a host that actually serves that agent protocol for the caller — on AWS the ECS agent connects to the returned endpoint and polls it. The recording asserts these members by value class rather than literally (the per-environment host is not itself a requirement), and the replay classifies the emulator's values as host_not_own: they name hosts outside the emulator's own serving address.
Emulator: it returns the real AWS production host family built from a per-region template — endpoint 'https://ecs-a-1.<region>.amazonaws.com/', telemetryEndpoint 'https://ecs-t-1.<region>.amazonaws.com/' and serviceConnectEndpoint 'https://ecs-sc.<region>.amazonaws.com/' — none of which the emulator serves. Status is 200 on both sides; only these three members diverge.
Why: this operation exists so an agent can learn where to poll. Announcing an address other than the one the emulator is reached on sends every ECS agent (and any client that honours the response) at AWS's production endpoints or at a name that does not resolve, so agent polling, telemetry submission and Service Connect configuration retrieval all fail against the emulator even though the discovery call itself succeeded.
Fix: the DiscoverPollEndpoint handler's response construction and the endpoint derivation it calls in the ecs service crate — derive all three endpoints from the emulator's own externally reachable address/port rather than from a real-AWS per-region host template. The concrete host and port are environment-specific and are not the requirement; serving the announced address is.
Traces: ecs-agent-apis-and-exec-error-paths#task#2 (same cluster also covers ecs-cluster-setup-poll-endpoint-and-teardown#task#2).

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (118 fixtures, 1380 steps, 1206 passed, score 87.4%). The cluster is neither blocked by an unimplemented-operation stub nor owned by an open issue's replay key.

DiscoverPollEndpoint, fixture ecs-agent-apis-and-exec-error-paths step task#2 (cluster also covers ecs-cluster-setup-poll-endpoint-and-teardown#task#2), expected 200 / actual 200:
  $.endpoint: host_not_own (expected '<shaped-not-asserted>', got 'https://ecs-a-1.us-west-2.amazonaws.com/')
  $.telemetryEndpoint: host_not_own (expected '<shaped-not-asserted>', got 'https://ecs-t-1.us-west-2.amazonaws.com/')
  $.serviceConnectEndpoint: value_mismatch (expected '<shaped-not-asserted>', got 'https://ecs-sc.us-west-2.amazonaws.com/')

The region in the recorded hosts is a per-run value; the defect is that the announced hosts belong to real AWS rather than to the address the emulator serves.

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay trace set simparity-wire-aws-v1.
2. Replay fixture ecs-agent-apis-and-exec-error-paths to step task#2 — create a cluster, register a container instance, then call DiscoverPollEndpoint for that cluster/container instance.
3. The response's endpoint, telemetryEndpoint and serviceConnectEndpoint are returned as https://ecs-a-1.<region>.amazonaws.com/, https://ecs-t-1.<region>.amazonaws.com/ and https://ecs-sc.<region>.amazonaws.com/ — hosts the emulator does not serve. Attempting to poll any of them from the same client that made the call does not reach the emulator.
