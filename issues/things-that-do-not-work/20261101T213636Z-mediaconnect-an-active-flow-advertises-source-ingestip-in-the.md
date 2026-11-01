Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediaconnect.native-media-ingest-udp-round-trip
Issue-Shape-Id: com.amazonaws.mediaconnect#MediaConnect
Issue-Key: mediaconnect.native-media-ingest-udp-round-trip|run#13|EgressRefused|mc, flow_arn, probe_socket, *ctx.endpoint_host(ingest_ip, ingest_port), payload, timeout_s=120
Issue-Commit: 98b98c5be76aaa007f7491e7e1444847b4627d62

# An ACTIVE flow advertises Source.IngestIp in the 198.51.100.0/24 documentation range, an address the emulator does not serve

> _Note: this issue was identified against the code base at commit `98b98c5be76aaa007f7491e7e1444847b4627d62`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFlow

## Description
One net-new root cause found replaying the behavioral suite against the MediaConnect emulator.

NOTE ON OVERLAP: the open issue 20261022T132818Z-mediaconnect-router-interface-union-payload-rejected-no-flow.md lists this check and its Cause 5 describes a DIFFERENT failure of the same check — "no native UDP listener exists and Source.PeerIpAddress never populates", keyed on the `_poll_for_peer_ip#11|Timeout` step. That is not what the check fails on now: it no longer reaches the peer-IP poll at all, it fails earlier on the ADDRESS the emulator advertises, and the replay report does not report this failure as owned. The address-announcement defect below is therefore net-new and is not covered by that issue (fixing the listener alone cannot close it, because the announced address is not one the emulator can listen on).

=== Cause 1 of 1: the ingest endpoint a flow advertises is a documentation-range address, not the address the emulator is reached on ===
AWS: Smithy model services/service-com-amazonaws-mediaconnect-mediaconnect/smithy-model/com-amazonaws-mediaconnect-mediaconnect.json — `Source$IngestIp` (jsonName `ingestIp`) is documented as "The IP address that the flow will be listening on for incoming content", paired with `Source$IngestPort` ("The port that the flow will be listening on for incoming content"), and `Source$PeerIpAddress` is "The IP address of the device that is currently sending content to this source". Beyond the model this is recorded behavior only: MediaConnect's data plane is raw UDP, so once a flow is ACTIVE the service binds exactly the endpoint it advertises on DescribeFlow as Source.IngestIp:IngestPort, and a sender that puts packets on that endpoint is observed there (its address appears on Source.PeerIpAddress). The requirement the model and the recording share is that the advertised endpoint is the endpoint the service actually listens on — IngestIp is an address a client is told to send content to, not an opaque label. For an emulator that means the address it announces must be the address it serves its data plane on; any other value is unreachable by construction.
Emulator: the flow source materialisation mints IngestIp from the 198.51.100.0/24 documentation range (TEST-NET-2) keyed off the ARN counter, independent of where the emulator is actually reachable — see services/service-com-amazonaws-mediaconnect-mediaconnect/src/state.rs, `StoredSource::from_set_request` calling `assigned_source_ingest_endpoint(..., Some(assigned_ingest_ip(suffix)), ...)` (~lines 1815-1833), and the crate's own comment at ~line 1929 describing it as "the same documentation range a source's own ingest address is drawn from ([`assigned_ingest_ip`])". CreateFlow + StartFlow succeed and the flow reaches ACTIVE; DescribeFlow then reports a Source.IngestIp in that range (observed value `198.51.100.3`, with the documentation-range prefix being the general behavior and the final octet a per-run counter artifact), while the emulator itself is reached on the endpoint under test. The harness refuses to transmit to the announced address: `simparity.egress.EgressRefused: egress refused: '198.51.100.3' is not the endpoint under test (127.0.0.1); an emulator run may reach only the emulator. If the host came from a URL the emulator returned, the emulator announces an address other than the endpoint it is reached on`.
Why: a client that does what the API tells it to do — read the ACTIVE flow's advertised ingest endpoint and send content there — sends to an address nobody serves. On a host with no route the datagrams are silently dropped; on a host with a route they leave for a third party in a reserved documentation range. The data-plane round trip cannot even be attempted, Source.PeerIpAddress can never populate, and the caller has no way to distinguish "the service advertised an address it does not own" from "my content is not arriving": there is no error, only silence. Every ingest, failover and monitoring workflow that keys on the advertised endpoint is unusable.
Fix: the flow-source ingest-endpoint assignment in this service's state — `assigned_ingest_ip` / `assigned_source_ingest_endpoint` as used by `StoredSource::from_set_request` in src/state.rs — must advertise the address on which the emulator actually serves the MediaConnect flow data plane, and that endpoint must be bound when the flow becomes ACTIVE (the flow start path, src/ops/start_flow.rs). The same relation feeds `InputConfiguration$InputIp` for media-stream source configurations, which is drawn from the same documentation range and has the same defect.
Traces: mediaconnect.native-media-ingest-udp-round-trip.

## Evidence
Replay report: mediaconnect 15/28 passed (54%), 13 failed; 12 of the 13 are marked OWNED by an existing issue. The surviving failure:

[error] mediaconnect.native-media-ingest-udp-round-trip
  steps: create_flow(Name=..., Source={'Name':'source1','Protocol':'rtp','WhitelistCidr':<runner egress CIDR>}) -> start_flow -> poll describe_flow until Status=ACTIVE -> read Flow.Source.IngestIp / IngestPort -> send 188-byte UDP probe packets to that endpoint.
  expected: the advertised Source.IngestIp:IngestPort is the endpoint the emulator serves, so the probe packets reach it and DescribeFlow subsequently reports Source.PeerIpAddress.
  actual:   DescribeFlow advertises an ingest address in the 198.51.100.0/24 documentation range (observed 198.51.100.3) while the emulator is reached on the endpoint under test, and the transmit is refused before any packet is sent:
            File "egress.py", line 248, in require
              refuse(host, (urlsplit(endpoint).hostname or "").lower())
            File "egress.py", line 238, in refuse
              raise EgressRefused(
            simparity.egress.EgressRefused: egress refused: '198.51.100.3' is not the endpoint under test (127.0.0.1); an emulator run may reach only the emulator. If the host came from a URL the emulator returned, the emulator announces an address other than the endpoint it is reached on

Source of the advertised value (services/service-com-amazonaws-mediaconnect-mediaconnect/src/state.rs):
  ~1815: let (ingest_ip, ingest_port) = assigned_source_ingest_endpoint(
  ~1818:     Some(assigned_ingest_ip(suffix)),
  ~1929: doc comment: "...from the same documentation range a source's own ingest address is drawn from ([`assigned_ingest_ip`])"

Not verified by this run (the check never got past the ingest transmit): its trailing assertions that CreateFlow with Source.Protocol='udp' is refused with BadRequestException whose message names the protocol invalid, and that the refused flow does not appear in ListFlows.

## Steps to Reproduce
1. Boot the emulator and point a botocore mediaconnect client at its endpoint.
2. flow = mc.create_flow(Name='f1', Source={'Name':'source1','Protocol':'rtp','WhitelistCidr':'0.0.0.0/0'})
3. mc.start_flow(FlowArn=flow['Flow']['FlowArn']); poll mc.describe_flow until Flow.Status == 'ACTIVE'.
4. src = mc.describe_flow(FlowArn=...)['Flow']['Source']; print(src['IngestIp'], src['IngestPort']).
   Observe IngestIp is in the 198.51.100.0/24 documentation range — not the host/port the emulator is reached on.
5. Send UDP datagrams from a plain stdlib socket to (IngestIp, IngestPort): nothing serves that address, so the packets go nowhere (the suite harness refuses the transmit outright with EgressRefused because the announced host is not the endpoint under test), and polling DescribeFlow for Source.PeerIpAddress never populates.
6. Equivalently: run run_behavioral_suite and observe mediaconnect.native-media-ingest-udp-round-trip erroring in egress.py require/refuse.
