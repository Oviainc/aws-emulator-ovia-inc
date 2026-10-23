Issue-Source: trace-sweep-divergence
Issue-Trace: globalaccelerator-accelerator-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: globalaccelerator-accelerator-lifecycle globalaccelerator-listener-lifecycle globalaccelerator-endpoint-group-lifecycle
Issue-Shape-Id: com.amazonaws.globalaccelerator#GlobalAccelerator_V20180706
Issue-Key: globalaccelerator|DescribeAccelerator|2xx->2xx|$.Accelerator.IpSets[*].IpAddressDetails
Issue-Key: globalaccelerator|UpdateAccelerator|2xx->2xx|$.Accelerator.IpSets[*].IpAddressDetails
Issue-Key: globalaccelerator|ListListeners|2xx->2xx|$.Listeners[*].ClientAffinity
Issue-Key: globalaccelerator|ListListeners|2xx->2xx|$.Listeners[*].PortRanges[*]
Issue-Key: globalaccelerator|ListListeners|2xx->2xx|$.Listeners[*].PortRanges[*].FromPort
Issue-Key: globalaccelerator|ListListeners|2xx->2xx|$.Listeners[*].PortRanges[*].ToPort
Issue-Key: globalaccelerator|CreateEndpointGroup|2xx->2xx|$.EndpointGroup.EndpointGroupArn
Issue-Commit: 326b33f63f33ac22c419b7169b10d47cfd8c011f

# Global Accelerator resource records returned incomplete, stale and with a short endpoint-group id

> _Note: this issue was identified against the code base at commit `326b33f63f33ac22c419b7169b10d47cfd8c011f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAccelerator

## Description
Four clusters of 200-OK responses whose resource records do not match the recorded AWS records: a modelled sub-structure is never emitted, a listener's configuration is reported as something other than what the fixture configured, and the generated endpoint-group id in an ARN has the wrong width. All are response/state fidelity faults in the same service's resource projection, and all four must be closed.

Cause 1 of 4 — DescribeAccelerator never emits IpSets[].IpAddressDetails. Fixture globalaccelerator-accelerator-lifecycle, step task#2 (cluster also covers task#5).
AWS: Smithy: Accelerator.IpSets -> IpSets -> IpSet, whose members are IpFamily (deprecated), IpAddresses (list, length 0-2), IpAddressFamily (enum) and IpAddressDetails (list IpAddressDetail, length 0-2, "The array of IP addresses in the IP address set, with detailed information about the IP addresses"). The recording returns, for each IpSet, an IpAddressDetails list parallel to IpAddresses, each entry carrying an IpAddress plus a NetworkZone (recorded zones: nz1, nz2 — the zone labels themselves are environment-specific, the presence of one per allocated address is not).
Emulator: the member is absent from every IpSet (detail: "$.Accelerator.IpSets[0].IpAddressDetails: missing (... got None)"), while IpAddresses is present and passes.
Why: a client reading network-zone information for an accelerator's static addresses — the only place the per-address zone is exposed — gets a null list and concludes the accelerator has no address details, so zone-aware logic and any SDK code dereferencing ipSets[].ipAddressDetails breaks or silently degrades.
Fix: the Accelerator/IpSet projection in the DescribeAccelerator response serializer, backed by the accelerator state that must record a NetworkZone per allocated static address.

Cause 2 of 4 — UpdateAccelerator has the same omission. Fixture globalaccelerator-accelerator-lifecycle, step task#4 (cluster also covers task#10).
AWS: same IpSet shape as Cause 1; UpdateAccelerator returns the full Accelerator record, including IpSets[].IpAddressDetails with one entry per static address.
Emulator: "$.Accelerator.IpSets[0].IpAddressDetails: missing (... got None)" on the UpdateAccelerator response.
Why: a client that reads the updated record straight out of the mutating call (the normal pattern, rather than a follow-up describe) sees no address details at all, so it cannot confirm the address/zone layout survived the update.
Fix: the shared Accelerator projection used by the UpdateAccelerator handler — it is a second call site of the same incomplete IpSet serialization, so verify both after the fix.

Cause 3 of 4 — ListListeners reports listener configuration that is not the one the fixture established. Fixture globalaccelerator-listener-lifecycle, step task#5.
AWS: Smithy: Listener = { ListenerArn, PortRanges (list, length 1-10), Protocol, ClientAffinity enum NONE|SOURCE_IP, default NONE }; ListListeners returns the listeners of an accelerator as currently configured. The recording shows the listener with ClientAffinity SOURCE_IP and two port ranges, 443-443 and 8080-8081, i.e. the configuration set by the preceding create/update steps of the fixture.
Emulator: ClientAffinity NONE, a single port range 80-80, and no second range (details: "$.Listeners[0].ClientAffinity: value_mismatch (expected 'SOURCE_IP', got 'NONE')", "PortRanges[0].FromPort: expected 443, got 80", "PortRanges[0].ToPort: expected 443, got 80", "PortRanges[1]: missing (expected {'FromPort': 8080, 'ToPort': 8081}, got None)"). The listener is listed, so the record exists — its ClientAffinity has fallen back to the enum default and its PortRanges list has been left at a single stale range instead of the configured multi-range list.
Why: this is silent configuration loss. A client that configures source-IP affinity and a multi-range listener and then reads the listener back is told its affinity is NONE and its ports are something else, so convergence loops (Terraform/CDK-style diff-and-apply) see permanent drift and re-apply forever, and anything asserting on the listener's ports reads a port the fixture never asked for.
Fix: the listener state write path plus the Listener projection used by ListListeners — the configured ClientAffinity and the full PortRanges list must be persisted on create/update and returned verbatim by the list operation.

Cause 4 of 4 — CreateEndpointGroup builds EndpointGroupArn with an 8-hex-character endpoint-group id instead of 12. Fixture globalaccelerator-endpoint-group-lifecycle, step task#3.
AWS: recorded behavior only — the model types EndpointGroup.EndpointGroupArn as a generic string and does not state the id format. The recording shows the final ARN segment as a 12-character lowercase hex id ('.../endpoint-group/8bec300ec005'), whereas the accelerator UUID and the 8-hex listener id earlier in the same ARN are reproduced exactly by the emulator, so the width of this one segment is the only thing that differs.
Emulator: an 8-character hex id ('.../endpoint-group/48d4557a'), i.e. the listener id width reused for the endpoint group (detail: "$.EndpointGroup.EndpointGroupArn: value_mismatch (expected '.../endpoint-group/8bec300ec005', got '.../endpoint-group/48d4557a')"). Hypothesis: the endpoint-group id generator shares the listener id generator's 4-byte width.
Why: the particular id value is environment-specific and is not a requirement, but its FORMAT is what clients parse. Code or fixtures that validate/round-trip an endpoint-group ARN against a 12-hex-character final segment reject the emulator's ARN, and ARN-length-sensitive consumers see a shape AWS never emits.
Fix: the endpoint-group id generator used when CreateEndpointGroup mints EndpointGroupArn — widen it to the recorded 12 hex characters. Severity of this cause alone is LOW; the bundle takes the HIGH of Cause 3.

Traces: globalaccelerator-accelerator-lifecycle#task#2, globalaccelerator-accelerator-lifecycle#task#4, globalaccelerator-listener-lifecycle#task#5, globalaccelerator-endpoint-group-lifecycle#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (all four steps returned HTTP 200, as recorded — the divergence is in the body):

1) globalaccelerator-accelerator-lifecycle#task#2 DescribeAccelerator — detail: "$.Accelerator.IpSets[0].IpAddressDetails: missing (expected [{'IpAddress': '<shaped-not-asserted>', 'NetworkZone': 'nz1'}, {'IpAddress': '<shaped-not-asserted>', 'NetworkZone': 'nz2'}], got None)". affected_count 2 (also task#5).
2) globalaccelerator-accelerator-lifecycle#task#4 UpdateAccelerator — same detail, affected_count 2 (also task#10). divergence_key service=globalaccelerator|UpdateAccelerator|status=200->200|keys=$.Accelerator.IpSets[0].IpAddressDetails
3) globalaccelerator-listener-lifecycle#task#5 ListListeners — detail: "$.Listeners[0].ClientAffinity: value_mismatch (expected 'SOURCE_IP', got 'NONE')\n$.Listeners[0].PortRanges[0].FromPort: value_mismatch (expected 443, got 80)\n$.Listeners[0].PortRanges[0].ToPort: value_mismatch (expected 443, got 80)\n$.Listeners[0].PortRanges[1]: missing (expected {'FromPort': 8080, 'ToPort': 8081}, got None)".
4) globalaccelerator-endpoint-group-lifecycle#task#3 CreateEndpointGroup — detail: "$.EndpointGroup.EndpointGroupArn: value_mismatch (expected 'arn:aws:globalaccelerator::000000000000:accelerator/896a5cf7-0df3-4eaf-90ba-d1ccbf7e6575/listener/2f9a4bf1/endpoint-group/8bec300ec005', got 'arn:aws:globalaccelerator::000000000000:accelerator/896a5cf7-0df3-4eaf-90ba-d1ccbf7e6575/listener/2f9a4bf1/endpoint-group/48d4557a')" — accelerator UUID and listener id match exactly; only the endpoint-group segment differs, 8 hex chars vs 12.

Replay summary: steps_total 76, passed 23, mismatched 37, state_reset_failures 0, so these bodies were produced against correctly reset per-fixture state.

Model evidence (smithy_query on services/service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706/smithy-model/com-amazonaws-globalaccelerator-globalaccelerator-v20180706.json): IpSet members include IpAddressDetails (list IpAddressDetail, length 0-2); Listener = { ListenerArn, PortRanges length 1-10, Protocol, ClientAffinity } with ClientAffinity enum NONE|SOURCE_IP documented default NONE; CreateEndpointGroup output EndpointGroup.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the corpus: replay_wire_corpus() for trace set simparity-wire-aws-v1.
2. Cause 1: run fixture globalaccelerator-accelerator-lifecycle to step task#2 — CreateAccelerator then DescribeAccelerator. Inspect Accelerator.IpSets[0]: IpAddresses is present, IpAddressDetails is absent. AWS returns one IpAddressDetails entry (IpAddress + NetworkZone) per static address. Step task#5 is in the same cluster.
3. Cause 2: continue the same fixture to step task#4 — UpdateAccelerator. Accelerator.IpSets[0].IpAddressDetails is again absent in the mutating call's own response. Step task#10 is in the same cluster.
4. Cause 3: run fixture globalaccelerator-listener-lifecycle to step task#5 — create/update the listener as the fixture does (ClientAffinity SOURCE_IP, port ranges 443-443 and 8080-8081), then ListListeners. The returned listener reports ClientAffinity NONE and a single 80-80 port range.
5. Cause 4: run fixture globalaccelerator-endpoint-group-lifecycle to step task#3 — CreateEndpointGroup. The returned EndpointGroupArn ends in an 8-hex-character id; AWS's ends in a 12-hex-character id.
6. All four steps must replay clean before closing; re-check task#5/task#10 of the accelerator fixture too, since they are clustered with Causes 1 and 2.
