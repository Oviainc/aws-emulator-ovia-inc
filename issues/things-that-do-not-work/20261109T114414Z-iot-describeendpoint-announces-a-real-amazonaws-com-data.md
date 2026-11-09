Issue-Source: trace-sweep-divergence
Issue-Trace: iot-domain-configuration-inspection
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-domain-configuration-inspection iot-event-configuration-and-endpoint-settings
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|DescribeEndpoint|2xx->2xx|$.endpointAddress
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# iot DescribeEndpoint announces a real amazonaws.com data endpoint the emulator does not serve

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEndpoint

## Description
Cause 1 of 1: DescribeEndpoint returns an endpointAddress pointing at real AWS instead of at the host the emulator is reached on.
AWS: Smithy DescribeEndpoint (GET /endpoint) returns `endpointAddress`, documented as 'The endpoint. The format of the endpoint is as follows: identifier.iot.region.amazonaws.com', where the identifier is the account's own endpoint identifier, created on the first call and returned unchanged by every later call. Recorded behavior only for the value itself: the recording shows a 200 with an `<identifier>-ats.iot.<region>.amazonaws.com` address, and because that identifier is an account-specific generated id it is NOT part of the requirement — the requirement is that the address a caller is handed is the address at which that caller can then reach this account's IoT data endpoint.
Emulator: the handler derives an identifier from the emulator account id plus the region and renders `<derived-identifier>-ats.iot.<region>.amazonaws.com` (services/service-com-amazonaws-iot-awsiotservice/src/ops/describe_endpoint.rs, EndpointType::address, lines 183-192, called at line 349). The replay flags the returned host as host_not_own: the announced host is not a host the emulator serves — it is a real AWS public DNS name, not the address the request itself arrived on.
Why: a client that does the normal thing — call DescribeEndpoint and then connect its MQTT/HTTPS data client to the address it was told — leaves the emulator entirely and egresses toward (or fails to resolve) real AWS. The whole IoT data plane is therefore unreachable by the documented discovery path, and the failure surfaces as a connection/DNS error far from the call that caused it, with no indication that the emulator handed out an address it does not answer on.
Fix: the address rendering in DescribeEndpoint's handler — it must announce the emulator's own reachable host (the host/port this request was served on, as the other self-referential endpoint surfaces do) while keeping the AWS-documented `identifier[-ats][.credentials|.jobs].iot.<region>...` shape and the create-once/return-same identifier semantics. services/service-com-amazonaws-iot-awsiotservice/src/ops/describe_endpoint.rs (EndpointType::address and the handler at lines 321-363).
Traces: iot-domain-configuration-inspection#task#1, iot-event-configuration-and-endpoint-settings#task#2 (same cluster).

Related but distinct: open issue 20261106T201842Z-iot-refusal-fidelity-10-net-new-causes-createpolicy.md, cause 5, concerns DescribeEndpoint's missing endpointType VALIDATION (iot:Jobs and unknown types accepted) and carries none of this cluster's replay keys; nothing filed covers the address it hands back.

## Evidence
Replayed wire corpus simparity-wire-aws-v1, fixture iot-domain-configuration-inspection step task#1 (cluster also covers iot-event-configuration-and-endpoint-settings#task#2):

DescribeEndpoint(endpointType='iot:Data-ATS') — status 200 == 200; body $.endpointAddress verdict host_not_own: expected a '<identifier>-ats.iot.us-west-2.amazonaws.com' address, got 'jzdx8mw2jjhboa-ats.iot.us-west-2.amazonaws.com', a host the emulator does not serve (the replay's host_not_own classification: the announced host is not one owned by the responding emulator, which is reached on its own data listener).

The recorded identifier itself is an account-specific generated id and is not a requirement; the defect is that the address returned is not an address at which the emulator answers, so the discovered data endpoint is unusable.

## Steps to Reproduce
Replay the wire fixtures from trace set simparity-wire-aws-v1:
1. iot-domain-configuration-inspection, step task#1 — call DescribeEndpoint(endpointType='iot:Data-ATS'); inspect endpointAddress. It must be an address that resolves and connects back to the emulator serving the call, not a *.iot.<region>.amazonaws.com public AWS host.
2. iot-event-configuration-and-endpoint-settings, step task#2 — same call, same expectation; a second call must return the same address as the first.
