Issue-Source: trace-sweep-divergence
Issue-Trace: globalaccelerator-accelerator-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: globalaccelerator-accelerator-lifecycle globalaccelerator-custom-routing-separation
Issue-Shape-Id: com.amazonaws.globalaccelerator#GlobalAccelerator_V20180706
Issue-Key: globalaccelerator|DescribeAccelerator|2xx->2xx|$.Accelerator.DnsName
Issue-Key: globalaccelerator|UpdateAccelerator|2xx->2xx|$.Accelerator.DnsName
Issue-Key: globalaccelerator|CreateCustomRoutingAccelerator|2xx->2xx|$.Accelerator.DnsName
Issue-Key: globalaccelerator|DescribeCustomRoutingAccelerator|2xx->2xx|$.Accelerator.DnsName
Issue-Commit: 82a95c6eb540e273b0e8819fa03b8f3bfa792799

# globalaccelerator accelerator DnsName announces the real awsglobalaccelerator.com host instead of a host the emulator serves

> _Note: this issue was identified against the code base at commit `82a95c6eb540e273b0e8819fa03b8f3bfa792799`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCustomRoutingAccelerator

## Description
4 net-new causes, all one shared fault: every accelerator-returning operation emits `Accelerator.DnsName` under the REAL AWS zone `awsglobalaccelerator.com`, a host the emulator does not own and does not serve. The replay checker flags each of them `host_not_own`: the name the emulator hands a client resolves (if at all) to AWS's production Global Accelerator fleet, never to the emulator's own data listener. The DnsName is minted once in the service state (`crate::state::dns_name` / `dual_stack_dns_name`, used by create_accelerator.rs:263-264 and the custom-routing create path) and then echoed by every read and update path, which is why the same fault shows up on four operations.

Cause 1 of 4: DescribeAccelerator. AWS: Smithy `Accelerator$DnsName` (GenericString, @length 0..255) documents the naming convention "A lowercase letter a, followed by a 16-bit random hex string, followed by .awsglobalaccelerator.com" and states it is "The Domain Name System (DNS) name that Global Accelerator creates that points to an accelerator's static IPv4 addresses" — i.e. a name that actually resolves to the addresses the SAME response advertises in `IpSets`. The recorded step does not assert the literal name (expected `<shaped-not-asserted>`), only that the announced host is one the responding endpoint owns. Emulator: DescribeAccelerator answers 200 with `$.Accelerator.DnsName` set to a `<label>.awsglobalaccelerator.com` host that belongs to AWS, not to the emulator. Why: a client that takes the DnsName from a Describe response and connects to it leaves the emulator entirely — it either hits real AWS Global Accelerator or fails to resolve, so accelerator-addressed traffic in a test never reaches the emulated endpoint groups and the whole accelerator data path is untestable. Fix: the DnsName minting in the globalaccelerator service state, so the name is formed from the host/zone the emulator is reached on (and the same for `DualStackDnsName`), with the read paths continuing to echo the stored value. Traces: globalaccelerator-accelerator-lifecycle#task#2 (also task#5).

Cause 2 of 4: UpdateAccelerator. AWS: same `Accelerator$DnsName` contract — an update returns the accelerator's unchanged, resolvable DNS name; the recording asserts only that the host is the responder's own. Emulator: UpdateAccelerator answers 200 and echoes the stored `<label>.awsglobalaccelerator.com` host, so the divergence survives mutation as well as creation. Why: a client that re-reads the DnsName after an update (the normal way to pick up a name change) is handed the same non-emulator host, so no amount of re-reading recovers a usable address. Fix: same DnsName minting/storage in the globalaccelerator state; Update must not re-introduce the AWS zone when it refreshes the record. Traces: globalaccelerator-accelerator-lifecycle#task#4 (also task#10).

Cause 3 of 4: CreateCustomRoutingAccelerator. AWS: Smithy `CustomRoutingAccelerator$DnsName` carries the same documentation and the same convention ending in `.awsglobalaccelerator.com`; it is the name AWS creates pointing at that accelerator's static IPv4 addresses. Recording asserts the host is the responder's own, not a literal value. Emulator: the create answers 200 with `$.Accelerator.DnsName` under `awsglobalaccelerator.com`. Why: the create response is where a client first learns how to address the accelerator, so a custom-routing test is handed an unreachable host at the very first step. Fix: the custom-routing accelerator create handler's DnsName minting (shared state helper), using the emulator's own host. Traces: globalaccelerator-custom-routing-separation#task#1.

Cause 4 of 4: DescribeCustomRoutingAccelerator. AWS: same `CustomRoutingAccelerator$DnsName` contract. Emulator: the describe answers 200 and echoes the same `awsglobalaccelerator.com` host stored at create time. Why: as cause 1, for custom-routing accelerators — the read path offers no corrected name. Fix: as cause 3; once the stored value names the emulator's host this read path is correct unchanged. Traces: globalaccelerator-custom-routing-separation#task#2.

Not covered by any open issue: 20261029T171730Z-globalaccelerator-error-messages-diverge-from-aws-wording-across.md covers error WORDING on validation/conflict paths and 20261104T101344Z-globalaccelerator-describeaccelerator-on-a-custom-routing-accelerator-returns.md covers the custom-routing separation MESSAGE; neither touches the DnsName the emulator announces on 200 responses.

## Evidence
Replay of trace set simparity-wire-aws-v1 (4 net-new divergence clusters, all verdict=mismatch, expected_status 200 / actual_status 200):
- globalaccelerator-accelerator-lifecycle#task#2 DescribeAccelerator — `$.Accelerator.DnsName: host_not_own (expected '<shaped-not-asserted>', got 'ac9e0b7669eef4421.awsglobalaccelerator.com')`; affected: task#2, task#5.
- globalaccelerator-accelerator-lifecycle#task#4 UpdateAccelerator — same detail, same host; affected: task#4, task#10.
- globalaccelerator-custom-routing-separation#task#1 CreateCustomRoutingAccelerator — `host_not_own (... got 'a3cb7b983aae0426e.awsglobalaccelerator.com')`.
- globalaccelerator-custom-routing-separation#task#2 DescribeCustomRoutingAccelerator — same detail, same host.
Expected: a DnsName whose host is one the responding emulator owns and serves, formed per the modeled convention (lowercase 'a' + 16-bit random hex label) over the emulator's own zone. Actual: the label is well formed but the zone is the production AWS zone `awsglobalaccelerator.com`. The specific labels (`ac9e0b7669eef4421`, `a3cb7b983aae0426e`) are per-run generated values and are NOT part of the requirement — only the zone/host is.
Source: `crate::state::dns_name(&label)` / `dual_stack_dns_name(&label)` stored at services/service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706/src/ops/create_accelerator.rs:263-264 and echoed by `StoredAccelerator::to_wire`.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture globalaccelerator-accelerator-lifecycle step task#2 (DescribeAccelerator after a CreateAccelerator) and inspect `$.Accelerator.DnsName` — its host is under awsglobalaccelerator.com, not the emulator's.
3. Cause 2: same fixture, step task#4 (UpdateAccelerator) — the echoed DnsName has the same foreign host.
4. Cause 3: replay fixture globalaccelerator-custom-routing-separation step task#1 (CreateCustomRoutingAccelerator) and inspect `$.Accelerator.DnsName`.
5. Cause 4: same fixture, step task#2 (DescribeCustomRoutingAccelerator).
6. In each case try to resolve/connect to the returned DnsName: it does not resolve to the emulator's data listener, which is what the host_not_own check reports.
